#![no_std]
use aidoku::{
	Chapter, ContentRating, DeepLinkHandler, DeepLinkResult, DynamicListings, FilterValue, HashMap,
	Home, HomeComponent, HomeComponentValue, HomeLayout, Link, Listing, ListingProvider, Manga,
	MangaPageResult, MangaStatus, MangaWithChapter, MigrationHandler, NotificationHandler, Page,
	PageContent, Result, Source, Viewer, WebLoginHandler,
	alloc::{String, Vec, string::ToString, vec},
	helpers::uri::QueryParameters,
	imports::{
		defaults::{defaults_get, defaults_set_data},
		net::{Request, TimeUnit, set_rate_limit},
	},
	prelude::*,
};

mod auth;
mod chapter_metadata;
mod helpers;
mod models;
mod site;

use models::*;
use core::cell::RefCell;

const BASE_URL: &str = "https://asurascans.com";
const API_URL: &str = "https://api.asurascans.com/api";

struct AsuraScans {
	web: RefCell<site::HtmlClient>,
}

impl Source for AsuraScans {
	fn new() -> Self {
		set_rate_limit(2, 2, TimeUnit::Seconds);
		Self { web: RefCell::new(site::HtmlClient::new()) }
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut qs = QueryParameters::new();
		qs.push("page", Some(&page.to_string()));
		if query.is_some() {
			qs.push("q", query.as_deref());
		}

		for filter in filters {
			match filter {
				FilterValue::Sort {
					id,
					index,
					ascending,
				} => {
					qs.push(
						&id,
						Some(match index {
							0 => "update",
							1 => "popular",
							2 => "rating",
							3 => "name",
							4 => "newest",
							_ => "update",
						}),
					);
					if ascending {
						qs.push("order", Some("asc"));
					}
				}
				FilterValue::Select { id, value } => {
					qs.push(&id, Some(&value));
				}
				FilterValue::MultiSelect { id, included, .. } => {
					qs.push(&id, Some(&included.join(",")));
				}
				_ => continue,
			}
		}

		let url = format!("{BASE_URL}/browse?{qs}");
		let html = Request::get(url)?.html()?;

		let entries = html
			.select("#series-grid > .series-card")
			.map(|els| {
				els.filter_map(|el| {
					Some(Manga {
						key: el
							.select_first("a")?
							.attr("abs:href")
							.and_then(|url| helpers::get_manga_key(&url))?,
						title: el.select_first("h3")?.own_text()?,
						cover: el.select_first("img").and_then(|el| el.attr("abs:src")),
						..Default::default()
					})
				})
				.collect()
			})
			.unwrap_or_default();

		let has_next_page = html
			.select_first("button[aria-label=\"Next page\"].cursor-pointer")
			.is_some();

		Ok(MangaPageResult {
			entries,
			has_next_page,
		})
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let url = manga.url.as_deref()
			.filter(|url| url.starts_with(BASE_URL))
			.map(Into::into)
			.unwrap_or_else(|| helpers::get_manga_url(&manga.key));
		let (html, resolved_url) = self.web.borrow_mut().get_html(&url)?;

		if needs_details {
			manga.title = html
				.select_first("h1.text-xl.font-semibold")
				.and_then(|el| el.own_text())
				.unwrap_or(manga.title);
			manga.cover = html
				.select_first("div#desktop-cover-container img")
				.and_then(|el| el.attr("abs:src"));
			manga.artists = html.select("a[href^=/browse?artist]").map(|els| {
				els.filter_map(|el| el.text())
					.filter(|s| s != "_")
					.collect()
			});
			manga.authors = html.select("a[href^=/browse?author]").map(|els| {
				els.filter_map(|el| el.text())
					.filter(|s| s != "_")
					.collect()
			});
			manga.description = html
				.select_first("div#description-text")
				.and_then(|el| el.text());
			manga.url = Some(resolved_url.clone());
			manga.tags = html
				.select("a[href^=/browse?genres=]")
				.map(|els| els.filter_map(|el| el.text()).collect());
			manga.status = html
				.select_first(
					"div.flex.gap-3.pt-4.border-t > div:nth-child(1) > div > span.text-base",
				)
				.and_then(|el| el.text())
				.map(|s| match s.as_str() {
					"ongoing" => MangaStatus::Ongoing,
					"hiatus" => MangaStatus::Hiatus,
					"completed" => MangaStatus::Completed,
					"dropped" => MangaStatus::Cancelled,
					_ => MangaStatus::Unknown,
				})
				.unwrap_or_default();
			let tags = manga.tags.as_deref().unwrap_or_default();
			manga.content_rating = if tags
				.as_ref()
				.iter()
				.any(|e| matches!(e.as_str(), "Adult" | "Ecchi"))
			{
				ContentRating::Suggestive
			} else {
				ContentRating::Safe
			};
			manga.viewer = html
				.select_first(
					"div.flex.gap-3.pt-4.border-t > div:nth-child(2) > div > span.text-base",
				)
				.and_then(|el| el.text())
				.map(|s| match s.as_str() {
					"manhwa" => Viewer::Webtoon,
					"manhua" => Viewer::Webtoon,
					"mangatoon" => Viewer::RightToLeft,
					_ => Viewer::Webtoon,
				})
				.unwrap_or(Viewer::Webtoon);
		}

		if needs_chapters {
			// Chapter links are server-rendered on the series page. Unlike the
			// "First Chapter" shortcut, the chapter list entries start with "Chapter".
			let skip_locked = !defaults_get::<bool>("showLocked").unwrap_or(true);
			let is_subscribed = auth::is_subscribed();
			// Aidoku identifies the current chapter by comparing the full
			// Chapter record. Relative dates such as "2 days ago" must not
			// produce a different timestamp every time the manga is refreshed.
			let date_cache_key = format!("asurascans.chapter_dates.{}", manga.key);
			let mut date_cache: HashMap<String, i64> =
				defaults_get(&date_cache_key).unwrap_or_default();
			let mut published_dates = HashMap::<String, i64>::new();
			let mut seen = Vec::<String>::new();

			let mut chapters: Vec<Chapter> = html
				.select("a[href*='/chapter/']")
				.map(|links| {
					links
						.filter_map(|link| {
							let label = link.text()?;
							// Exclude "First Chapter", "Start Reading", and navigation
							// shortcuts, not the actual chapter list rows.
							if !label.trim_start().starts_with("Chapter") {
								return None;
							}

							let href = link.attr("abs:href").or_else(|| link.attr("href"))?;
							let key = helpers::get_chapter_key(&href)?;
							let chapter_number = key.parse::<f32>().ok()?;
							if seen.contains(&key) {
								return None;
							}

							let label_lower = label.to_ascii_lowercase();
							let locked = !is_subscribed
								&& (label_lower.contains("early access")
									|| label_lower.contains("unlocks in")
									|| link.select_first("svg.lucide-lock").is_some());
							seen.push(key.clone());

							let date_uploaded = helpers::get_chapter_date(&link);

							Some(Chapter {
								key,
								chapter_number: Some(chapter_number),
								date_uploaded,
								url: Some(href),
								locked,
								..Default::default()
							})
						})
						.collect()
				})
				.unwrap_or_default();

			// Prefer structured Astro metadata for access status: the anchor
			// often hides premium/early-access state in non-text attributes.
			for entry in chapter_metadata::from_html(&html) {
				let key = entry.number.to_string();
				let locked = entry.locked.unwrap_or(false) && !is_subscribed;
				if let Some(timestamp) = entry.date_uploaded {
					published_dates.insert(key.clone(), timestamp);
				}
				if let Some(chapter) = chapters.iter_mut().find(|chapter| chapter.key == key) {
					if entry.locked.is_some() {
						chapter.locked = locked;
					}
					if let Some(date_uploaded) = entry.date_uploaded {
						// Prefer the source's exact published date over a
						// display-only relative date.
						chapter.date_uploaded = Some(date_uploaded);
					}
					if chapter.title.is_none() {
						chapter.title = entry.title;
					}
				} else {
					// Some premium chapters are metadata-only for guests.
					chapters.push(Chapter {
						key: key.clone(),
						chapter_number: Some(entry.number),
						date_uploaded: entry.date_uploaded,
						title: entry.title,
						url: Some(format!("{}/chapter/{key}", resolved_url.trim_end_matches('/'))),
						locked,
						..Default::default()
					});
				}
			}
			// Keep a stable upload timestamp for every chapter across refreshes.
			// Exact dates from the Astro payload take priority, then prior
			// cached timestamps, then the approximate relative HTML date.
			for chapter in &mut chapters {
				let date_uploaded = published_dates.get(&chapter.key).copied()
					.or_else(|| date_cache.get(&chapter.key).copied())
					.or(chapter.date_uploaded);
				chapter.date_uploaded = date_uploaded;
				if let Some(timestamp) = date_uploaded {
					date_cache.insert(chapter.key.clone(), timestamp);
				}
			}
			defaults_set_data(&date_cache_key, date_cache);
			if skip_locked {
				chapters.retain(|chapter| !chapter.locked);
			}

			// Follow Aidoku's source convention: latest chapter first. This also
			// protects the order when the website adds shortcuts or changes markup.
			chapters.sort_by(|a, b| {
				b.chapter_number
					.partial_cmp(&a.chapter_number)
					.unwrap_or(core::cmp::Ordering::Equal)
			});

			if chapters.is_empty() {
				let page_title = html
					.select_first("title")
					.and_then(|el| el.text())
					.unwrap_or_default()
					.to_ascii_lowercase();
				if page_title.contains("just a moment")
					|| page_title.contains("attention required")
				{
					bail!("AsuraScans returned a Cloudflare challenge");
				}
				bail!("No chapter links in AsuraScans HTML. Website access or parsing failed.");
			}
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let chapter_url = chapter.url.as_deref()
			.filter(|url| url.starts_with(BASE_URL))
			.map(Into::into)
			.unwrap_or_else(|| helpers::get_chapter_url(&chapter.key, &manga.key));

		// The site serves page URLs in the browser-rendered chapter HTML.
		let (html, _) = self.web.borrow_mut().get_html_for_reading(&chapter_url)?;
		let pages: Vec<Page> = html
			.select("img[alt^='Page '], img[src*='/asura-images/chapters/'], img[data-src*='/asura-images/chapters/']")
			.map(|images| {
				images.filter_map(|img| {
					let url = img.attr("abs:data-src")
						.or_else(|| img.attr("abs:src"))
						.or_else(|| img.attr("data-src"))
						.or_else(|| img.attr("src"))?;
					if !url.starts_with("https://") && !url.starts_with("http://") {
						return None;
					}
					Some(Page {
						content: PageContent::url(url),
						..Default::default()
					})
				}).collect()
			})
			.unwrap_or_default();
		if !pages.is_empty() {
			return Ok(pages);
		}

		// Premium chapters may still need the authenticated API.
		let api_url = format!("{API_URL}/series/{}/chapters/{}", manga.key, chapter.key);
		let mut api_req = Request::get(api_url)?;
		if let Ok(status) = auth::get_login_status() {
			api_req.set_header("Authorization", &format!("Bearer {}", status.access_token));
			api_req.set_header("Cookie", &format!(
				"access_token={}; refresh_token={}",
				status.access_token, status.refresh_token
			));
		}
		if let Ok(json) = api_req.json_owned::<serde_json::Value>()
			&& let Some(page_arr) = json["data"]["chapter"]["pages"].as_array()
		{
			let pages: Vec<Page> = page_arr.iter().filter_map(|obj| {
				let url = obj.as_str()
					.or_else(|| obj["url"].as_str())
					.or_else(|| obj["url"][1].as_str())?;
				Some(Page {
					content: PageContent::url(url),
					..Default::default()
				})
			}).collect();
			if !pages.is_empty() {
				return Ok(pages);
			}
		}
		// Older versions of the site used an Astro reader island.
		if let Some(props) = html.select_first(
			"astro-island[component-url*=ChapterReader], astro-island[opts*=ChapterReader]"
		).and_then(|el| el.attr("props")) {
			if let Ok(json) = serde_json::from_str::<serde_json::Value>(&props) {
				if let Some(items) = json["pages"][1].as_array() {
					let pages: Vec<Page> = items.iter().filter_map(|obj| {
						let url = obj[1]["url"][1].as_str()?;
						Some(Page {
							content: PageContent::url(url),
							..Default::default()
						})
					}).collect();
					if !pages.is_empty() {
						return Ok(pages);
					}
				}
			}
		}
		bail!("AsuraScans returned no readable images for this chapter")
	}

}

impl Home for AsuraScans {
	fn get_home(&self) -> Result<HomeLayout> {
		let html = Request::get(BASE_URL)?.html()?;

		let mut components = Vec::new();

		if let Some(trending_today) =
			html.select_first("astro-island[opts*=TrendingSection] > section")
		{
			let title = trending_today
				.select_first("h2")
				.and_then(|el| el.text())
				.unwrap_or("Trending Today".into());
			let entries: Vec<Link> = trending_today
				.select("div.embla-trending > div > div > a")
				.map(|els| {
					els.filter_map(|el| {
						let key = helpers::get_manga_key(&el.attr("abs:href")?)?;
						Some(
							Manga {
								key,
								title: el.select_first("span.block")?.text()?,
								cover: el.select_first("img").and_then(|img| img.attr("abs:src")),
								..Default::default()
							}
							.into(),
						)
					})
					.collect()
				})
				.unwrap_or_default();
			if !entries.is_empty() {
				components.push(HomeComponent {
					title: Some(title),
					subtitle: None,
					value: HomeComponentValue::Scroller {
						entries,
						listing: None,
					},
				});
			}
		}

		if let Some(latest_updates) =
			html.select_first("astro-island[opts*=LatestUpdates] > section")
		{
			let title = latest_updates
				.select_first("h2")
				.and_then(|el| el.text())
				.unwrap_or("Latest Updates".into());
			let entries: Vec<MangaWithChapter> = latest_updates
				.select("div.grid > div.grid")
				.map(|els| {
					els.filter_map(|el| {
						let link = el.select_first("a.font-bold")?;
						let chapter_link = el.select_first("div.flex > div > a.group.grid")?;
						let manga_key = helpers::get_manga_key(&link.attr("abs:href")?)?;
						let chapter_key =
							helpers::get_chapter_key(&chapter_link.attr("abs:href")?)?;
						let chapter_number = chapter_link
							.select_first("span.font-medium")?
							.text()?
							.strip_prefix("Chapter")?
							.trim()
							.parse()
							.ok();
						Some(MangaWithChapter {
							manga: Manga {
								key: manga_key,
								title: link.text()?,
								cover: el.select_first("img").and_then(|img| img.attr("abs:src")),
								..Default::default()
							},
							chapter: Chapter {
								key: chapter_key,
								chapter_number,
								..Default::default()
							},
						})
					})
					.collect()
				})
				.unwrap_or_default();
			if !entries.is_empty() {
				components.push(HomeComponent {
					title: Some(title),
					subtitle: None,
					value: HomeComponentValue::MangaChapterList {
						page_size: None,
						entries,
						listing: None,
					},
				});
			}
		}

		Ok(HomeLayout { components })
	}
}

impl DeepLinkHandler for AsuraScans {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(manga_key) = helpers::get_manga_key(&url) else {
			return Ok(None);
		};

		if let Some(chapter_key) = helpers::get_chapter_key(&url) {
			Ok(Some(DeepLinkResult::Chapter {
				manga_key,
				key: chapter_key,
			}))
		} else {
			Ok(Some(DeepLinkResult::Manga { key: manga_key }))
		}
	}
}

impl MigrationHandler for AsuraScans {
	fn handle_manga_migration(&self, key: String) -> Result<String> {
		// v12: asuracomic.net -> asurascans.com, trailing '-' removed from ids
		Ok(key.strip_suffix("-").map(Into::into).unwrap_or(key))
	}

	fn handle_chapter_migration(&self, _manga_key: String, chapter_key: String) -> Result<String> {
		Ok(chapter_key) // no change
	}
}

impl ListingProvider for AsuraScans {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		match listing.id.as_str() {
			"Ranking" => {
				let html = Request::get(format!("{BASE_URL}/series-ranking"))?.html()?;
				let entries = html
					.select(".comics-ranking-list > a")
					.map(|els| {
						els.filter_map(|el| {
							// the ranking page doesn't have extra ids appended to the slug
							let key = el.attr("abs:href").and_then(|url| {
								url.split('/')
									.skip_while(|segment| *segment != "comics")
									.nth(1)
									.map(Into::into)
							})?;
							Some(Manga {
								key,
								title: el.select_first(".flex-1 > .text-sm")?.own_text()?,
								cover: el.select_first("img").and_then(|el| el.attr("abs:src")),
								..Default::default()
							})
						})
						.collect()
					})
					.unwrap_or_default();
				Ok(MangaPageResult {
					entries,
					has_next_page: false,
				})
			}
			"Bookmarks" => {
				let offset = 20 * (page - 1);
				let token = auth::get_access_token()?;
				let url = format!(
					"{API_URL}/me/bookmarks?sort=updated&order=desc&limit=20&offset={offset}",
				);
				let json: BookmarkResponse = Request::get(url)?
					.header("Authorization", &format!("Bearer {token}"))
					.json_owned()?;
				let entries = json.data.into_iter().map(Into::into).collect();
				let has_next_page = page < json.meta.total;
				Ok(MangaPageResult {
					entries,
					has_next_page,
				})
			}
			_ => bail!("Invalid listing"),
		}
	}
}

impl DynamicListings for AsuraScans {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		if !auth::is_logged_in() {
			return Ok(Vec::new());
		}
		Ok(vec![Listing {
			id: "Bookmarks".into(),
			name: "Bookmarks".into(),
			..Default::default()
		}])
	}
}

impl WebLoginHandler for AsuraScans {
	fn handle_web_login(&self, _key: String, cookies: HashMap<String, String>) -> Result<bool> {
		auth::handle_login(cookies)
	}
}

impl NotificationHandler for AsuraScans {
	fn handle_notification(&self, notification: String) {
		if notification != "login" {
			return;
		}
		let is_logged_in = defaults_get::<String>("login").is_some();
		if !is_logged_in {
			auth::logout();
		}
	}
}

register_source!(
	AsuraScans,
	Home,
	DeepLinkHandler,
	MigrationHandler,
	ListingProvider,
	DynamicListings,
	WebLoginHandler,
	NotificationHandler
);
