#![no_std]
use aidoku::{
	Chapter, ContentRating, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent,
	HomeComponentValue, HomeLayout, Link, LinkValue, Listing, ListingProvider, Manga,
	MangaPageResult, MangaStatus, MangaWithChapter, Page, PageContent, PageContext, Result, Source,
	Viewer,
	alloc::{String, Vec, string::ToString, vec},
	imports::{net::Request, html::Html},
	imports::std::{parse_date, send_partial_result},
	prelude::*,
};
use serde::Deserialize;
use core::cell::RefCell;

mod rsc;
mod network;
mod rendered;

use rsc::*;

const DOMAIN: &str = "https://templetoons.com";
const API_URL: &str = "https://api.templetoons.com/api";
const PAGE_SIZE: usize = 20;

#[derive(Deserialize, Clone, Default)]
struct BrowseSeries {
	series_slug: String,
	#[serde(default)]
	title: String,
	thumbnail: Option<String>,
	status: Option<String>,
	alternative_names: Option<String>,
	created_at: Option<String>,
	total_views: Option<serde_json::Value>,
	#[serde(rename = "Chapter")]
	chapter: Option<Vec<SeasonChapter>>,
}

#[derive(Deserialize, Clone, Default)]
struct SeasonChapter {
	chapter_slug: String,
	chapter_name: Option<String>,
	chapter_title: Option<String>,
	created_at: Option<String>,
	price: Option<f64>,
}

#[derive(Deserialize, Default)]
struct Season {
	#[serde(rename = "Chapter")]
	chapter: Option<Vec<SeasonChapter>>,
}

#[derive(Deserialize, Default)]
struct SeriesData {
	#[serde(default)]
	title: String,
	description: Option<String>,
	author: Option<String>,
	studio: Option<String>,
	status: Option<String>,
	thumbnail: Option<String>,
	tag_series: Option<Vec<TagSeries>>,
	#[serde(rename = "Season")]
	season: Option<Vec<Season>>,
}

#[derive(Deserialize)]
struct TagSeries {
	tag: Option<TagName>,
}

#[derive(Deserialize)]
struct TagName {
	name: Option<String>,
}

#[derive(Deserialize, Default)]
struct FeaturedEntry {
	series_slug: String,
	#[serde(default)]
	title: String,
	protagonist: Option<String>,
	description: Option<String>,
	total_views: Option<i64>,
}

#[derive(Deserialize, Default)]
struct TrendingEntry {
	series_slug: String,
	#[serde(default)]
	title: String,
	thumbnail: Option<String>,
}

#[derive(Deserialize, Default)]
struct TrendingResponse {
	#[serde(rename = "dayRes")]
	day: Option<Vec<TrendingEntry>>,
	#[serde(rename = "weekRes")]
	week: Option<Vec<TrendingEntry>>,
	#[serde(rename = "mensualRes")]
	month: Option<Vec<TrendingEntry>>,
}

/// Requests a route's flight stream rather than its rendered HTML.
fn fetch_rsc(web: &RefCell<network::BrowserClient>, url: &str) -> Result<String> {
	web.borrow_mut().get_rsc(url)
}

fn image_url(path: Option<&str>) -> Option<String> {
	let path = path?.trim();
	if path.is_empty() {
		return None;
	}
	Some(if path.starts_with("http") {
		path.to_string()
	} else if path.starts_with('/') {
		format!("{DOMAIN}{path}")
	} else {
		format!("{DOMAIN}/{path}")
	})
}

fn status_from(status: Option<&str>) -> MangaStatus {
	match status.unwrap_or("").to_lowercase().as_str() {
		s if s.contains("ongoing") => MangaStatus::Ongoing,
		s if s.contains("completed") => MangaStatus::Completed,
		s if s.contains("hiatus") => MangaStatus::Hiatus,
		s if s.contains("cancel") || s.contains("drop") => MangaStatus::Cancelled,
		_ => MangaStatus::Unknown,
	}
}

fn strip_html(html: &str) -> String {
	let mut out = String::with_capacity(html.len());
	let mut in_tag = false;
	let normalized = html.replace("<br>", "\n").replace("<br/>", "\n");
	for ch in normalized.chars() {
		match ch {
			'<' => in_tag = true,
			'>' => in_tag = false,
			_ => {
				if !in_tag {
					out.push(ch);
				}
			}
		}
	}
	// Trailing hashtag blocks are keyword spam rather than prose.
	let text = match out.find('#') {
		Some(idx) => out[..idx].trim_end_matches([' ', ':', '\n']).to_string(),
		None => out,
	};
	text.replace("&nbsp;", " ")
		.replace("&amp;", "&")
		.replace("&#39;", "'")
		.trim()
		.to_string()
}

fn parse_iso(value: Option<&str>) -> Option<i64> {
	let raw = value?.trim();
	if raw.len() < 19 {
		return None;
	}
	parse_date(raw[..19].replace('T', " "), "yyyy-MM-dd HH:mm:ss")
}

fn chapter_number_of(chapter: &SeasonChapter) -> Option<f32> {
	let name = chapter
		.chapter_name
		.as_deref()
		.unwrap_or(&chapter.chapter_slug);
	let digits: String = name
		.chars()
		.skip_while(|c| !c.is_ascii_digit())
		.take_while(|c| c.is_ascii_digit() || *c == '.')
		.collect();
	digits.trim_matches('.').parse::<f32>().ok()
}

fn browse_to_manga(series: BrowseSeries) -> Manga {
	Manga {
		key: series.series_slug,
		title: series.title,
		cover: image_url(series.thumbnail.as_deref()),
		status: status_from(series.status.as_deref()),
		content_rating: ContentRating::Safe,
		viewer: Viewer::Webtoon,
		..Default::default()
	}
}

fn link_of(key: String, title: String, cover: Option<String>) -> Link {
	let manga = Manga {
		key,
		title,
		cover,
		content_rating: ContentRating::Safe,
		viewer: Viewer::Webtoon,
		..Default::default()
	};
	Link {
		title: manga.title.clone(),
		subtitle: None,
		image_url: manga.cover.clone(),
		value: Some(LinkValue::Manga(manga)),
	}
}

/// The `/comics` route carries the whole catalogue in one array.
fn directory(web: &RefCell<network::BrowserClient>) -> Result<Vec<BrowseSeries>> {
	let payload = fetch_rsc(web, &format!("{DOMAIN}/comics"))?;
	let items: Vec<BrowseSeries> = largest_array(&payload, |items: &[BrowseSeries]| {
		items.first().map(|first| !first.series_slug.is_empty() && !first.title.is_empty())
			.unwrap_or(false)
	});
	if !items.is_empty() {
		return Ok(items);
	}
	// Newer TempleToons releases obfuscate the Flight JSON keys. Read the
	// browser-rendered cards instead of depending on those rotating keys.
	let doc = Html::parse_with_url(&payload, DOMAIN)?;
	let mut entries = Vec::<BrowseSeries>::new();
	if let Some(links) = doc.select("a[href*='/comic/']") {
		for link in links {
			let Some(title) = link.select_first("h2").and_then(|element| element.text()) else {
				continue;
			};
			let Some(url) = link.attr("abs:href").or_else(|| link.attr("href")) else {
				continue;
			};
			let Some(slug) = url.split("/comic/").nth(1).and_then(|value| value.split(['/', '?', '#']).next()) else {
				continue;
			};
			if slug.is_empty() || entries.iter().any(|entry| entry.series_slug == slug) {
				continue;
			}
			let thumbnail = link.parent()
				.and_then(|parent| parent.select_first("img"))
				.and_then(|image| image.attr("abs:src").or_else(|| image.attr("src")));
			entries.push(BrowseSeries {
				series_slug: slug.to_string(),
				title,
				thumbnail,
				..Default::default()
			});
		}
	}
	if entries.is_empty() {
		bail!("Temple Scan: no series cards in browser HTML. Site layout may have changed.");
	}
	Ok(entries)
}

fn page_of(items: Vec<BrowseSeries>, page: i32) -> MangaPageResult {
	let start = (page.max(1) as usize - 1) * PAGE_SIZE;
	let has_next_page = items.len() > start + PAGE_SIZE;
	MangaPageResult {
		entries: items
			.into_iter()
			.skip(start)
			.take(PAGE_SIZE)
			.map(browse_to_manga)
			.collect(),
		has_next_page,
	}
}

fn views_of(series: &BrowseSeries) -> f64 {
	match series.total_views.as_ref() {
		Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
		Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
		_ => 0.0,
	}
}

fn sort_directory(items: &mut [BrowseSeries], sort: i32) {
	match sort {
		1 => items.sort_by(|a, b| {
			let left = a.chapter.as_ref().and_then(|c| c.first());
			let right = b.chapter.as_ref().and_then(|c| c.first());
			parse_iso(right.and_then(|c| c.created_at.as_deref()))
				.cmp(&parse_iso(left.and_then(|c| c.created_at.as_deref())))
		}),
		2 => items.sort_by(|a, b| {
			parse_iso(b.created_at.as_deref()).cmp(&parse_iso(a.created_at.as_deref()))
		}),
		_ => items.sort_by(|a, b| {
			views_of(b)
				.partial_cmp(&views_of(a))
				.unwrap_or(core::cmp::Ordering::Equal)
		}),
	}
}

struct TempleScan {
	browser: RefCell<network::BrowserClient>,
}

impl Source for TempleScan {
	fn new() -> Self {
		Self { browser: RefCell::new(network::BrowserClient::new()) }
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let query = query.unwrap_or_default();
		let query = query.trim().to_lowercase();

		let mut sort = 0;
		let mut status = String::new();
		for filter in filters {
			match filter {
				FilterValue::Sort { index, .. } => sort = index,
				FilterValue::Select { id, value } if id == "status" => status = value,
				_ => {}
			}
		}

		let mut items = directory(&self.browser)?;
		items.retain(|series| {
			if !query.is_empty() {
				let title = series.title.to_lowercase();
				let alternative = series
					.alternative_names
					.as_deref()
					.unwrap_or("")
					.to_lowercase();
				if !title.contains(&query) && !alternative.contains(&query) {
					return false;
				}
			}
			if !status.is_empty()
				&& !series
					.status
					.as_deref()
					.unwrap_or("")
					.eq_ignore_ascii_case(&status)
			{
				return false;
			}
			true
		});
		sort_directory(&mut items, sort);
		Ok(page_of(items, page))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let slug = manga.key.clone();
		let payload = fetch_rsc(&self.browser, &format!("{DOMAIN}/comic/{slug}"))?;
		let Some(mut data) = extract_by_key::<SeriesData, _>(&payload, "seriesData", |value| {
			!value.title.is_empty()
		}) else {
			// The current site uses obfuscated Next.js props. Its visible
			// page has the stable title and chapter-link elements.
			return rendered::manga_update(manga, &payload, needs_details, needs_chapters);
		};

		if needs_details {
			// A bare "$row" description is an unresolved flight pointer.
			if let Some(description) = data.description.as_deref()
				&& description.starts_with('$')
			{
				data.description = resolve_flight_ref(&payload, description);
			}

			manga.title = data.title.clone();
			manga.cover = image_url(data.thumbnail.as_deref());
			manga.description = data
				.description
				.as_deref()
				.map(strip_html)
				.filter(|d| !d.is_empty());
			manga.authors = data
				.author
				.as_deref()
				.map(str::trim)
				.filter(|a| !a.is_empty())
				.map(|a| vec![a.to_string()]);
			manga.artists = data
				.studio
				.as_deref()
				.map(str::trim)
				.filter(|a| !a.is_empty())
				.map(|a| vec![a.to_string()]);
			manga.status = status_from(data.status.as_deref());
			manga.viewer = Viewer::Webtoon;
			manga.url = Some(format!("{DOMAIN}/comic/{slug}"));

			let tags: Vec<String> = data
				.tag_series
				.as_ref()
				.map(|tags| {
					tags.iter()
						.filter_map(|entry| entry.tag.as_ref()?.name.as_deref())
						.map(|name| name.trim().to_string())
						.filter(|name| !name.is_empty())
						.collect()
				})
				.unwrap_or_default();
			manga.content_rating = if tags.iter().any(|t| {
				let lower = t.to_lowercase();
				lower == "adult" || lower == "mature" || lower == "smut"
			}) {
				ContentRating::NSFW
			} else {
				ContentRating::Safe
			};
			manga.tags = (!tags.is_empty()).then_some(tags);

			if needs_chapters {
				send_partial_result(&manga);
			}
		}

		if needs_chapters {
			let mut chapters: Vec<Chapter> = data
				.season
				.unwrap_or_default()
				.into_iter()
				.flat_map(|season| season.chapter.unwrap_or_default())
				.filter(|chapter| !chapter.chapter_slug.is_empty())
				.map(|chapter| {
					let number = chapter_number_of(&chapter);
					let title = chapter
						.chapter_title
						.as_deref()
						.map(str::trim)
						.filter(|t| !t.is_empty())
						.map(|t| t.to_string());
					Chapter {
						url: Some(format!("{DOMAIN}/comic/{slug}/{}", chapter.chapter_slug)),
						key: chapter.chapter_slug,
						title,
						chapter_number: number,
						date_uploaded: parse_iso(chapter.created_at.as_deref()),
						locked: chapter.price.unwrap_or(0.0) > 0.0,
						language: Some("en".into()),
						..Default::default()
					}
				})
				.collect();
			chapters.sort_by(|a, b| {
				b.chapter_number
					.partial_cmp(&a.chapter_number)
					.unwrap_or(core::cmp::Ordering::Equal)
			});
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = format!("{DOMAIN}/comic/{}/{}", manga.key, chapter.key);
		let urls = self.browser.borrow_mut().get_reader_pages(&url)?;
		Ok(urls
			.into_iter()
			.filter_map(|url| image_url(Some(&url)))
			.map(|url| Page {
				content: PageContent::url(url),
				..Default::default()
			})
			.collect())
	}
}

impl Home for TempleScan {
	fn get_home(&self) -> Result<HomeLayout> {
		// The previous home ignored three failed requests and returned an empty
		// layout. Rely on one browser-backed catalogue request instead; errors
		// are surfaced to Aidoku rather than silently showing a blank page.
		let catalogue = directory(&self.browser)?;
		if catalogue.is_empty() {
			bail!("Temple Scan: empty comics catalogue");
		}

		let mut popular = catalogue.clone();
		sort_directory(&mut popular, 0);
		let featured: Vec<Manga> = popular
			.into_iter()
			.take(12)
			.map(browse_to_manga)
			.collect();

		let mut recent = catalogue.clone();
		sort_directory(&mut recent, 1);
		let updates: Vec<MangaWithChapter> = recent
			.into_iter()
			.filter_map(|series| {
				let chapter = series.chapter.as_ref()?
					.iter()
					.max_by_key(|item| parse_iso(item.created_at.as_deref()))?
					.clone();
				let chapter_number = chapter_number_of(&chapter);
				let date_uploaded = parse_iso(chapter.created_at.as_deref());
				let locked = chapter.price.unwrap_or(0.0) > 0.0;
				Some(MangaWithChapter {
					manga: browse_to_manga(series),
					chapter: Chapter {
						key: chapter.chapter_slug,
						chapter_number,
						date_uploaded,
						locked,
						..Default::default()
					},
				})
			})
			.take(15)
			.collect();

		let mut newest = catalogue;
		sort_directory(&mut newest, 2);
		let new_links: Vec<Link> = newest
			.into_iter()
			.take(15)
			.map(|series| {
				link_of(
					series.series_slug,
					series.title,
					image_url(series.thumbnail.as_deref()),
				)
			})
			.collect();

		let mut components = Vec::new();
		if !featured.is_empty() {
			components.push(HomeComponent {
				title: Some("Most Viewed".into()),
				subtitle: None,
				value: HomeComponentValue::BigScroller {
					entries: featured,
					auto_scroll_interval: Some(8.0),
				},
			});
		}
		if !updates.is_empty() {
			components.push(HomeComponent {
				title: Some("Latest Updates".into()),
				subtitle: None,
				value: HomeComponentValue::MangaChapterList {
					page_size: Some(5),
					entries: updates,
					listing: Some(Listing {
						id: "updated".into(),
						name: "Latest Updates".into(),
						..Default::default()
					}),
				},
			});
		}
		if !new_links.is_empty() {
			components.push(HomeComponent {
				title: Some("Recently Added".into()),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: new_links,
					listing: Some(Listing {
						id: "created".into(),
						name: "Newest".into(),
						..Default::default()
					}),
				},
			});
		}
		Ok(HomeLayout { components })
	}
}

impl ListingProvider for TempleScan {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let sort = match listing.id.as_str() {
			"views" => 0,
			"updated" => 1,
			"created" => 2,
			_ => bail!("Unknown listing"),
		};
		let mut items = directory(&self.browser)?;
		sort_directory(&mut items, sort);
		Ok(page_of(items, page))
	}
}

impl aidoku::ImageRequestProvider for TempleScan {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		Ok(Request::get(url)?.header("Referer", &format!("{DOMAIN}/")))
	}
}

impl DeepLinkHandler for TempleScan {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(idx) = url.find("/comic/") else {
			return Ok(None);
		};
		let mut segments = url[idx + "/comic/".len()..].split(['/', '?', '#']);
		let Some(slug) = segments.next().filter(|s| !s.is_empty()) else {
			return Ok(None);
		};
		if let Some(chapter) = segments.next().filter(|s| !s.is_empty()) {
			return Ok(Some(DeepLinkResult::Chapter {
				manga_key: slug.to_string(),
				key: chapter.to_string(),
			}));
		}
		Ok(Some(DeepLinkResult::Manga {
			key: slug.to_string(),
		}))
	}
}

register_source!(
	TempleScan,
	Home,
	ListingProvider,
	ImageRequestProvider,
	DeepLinkHandler
);
