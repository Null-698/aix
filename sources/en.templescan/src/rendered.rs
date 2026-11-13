//! Parse TempleToons pages by their rendered markup. The site changes
//! its internal Next.js field names on deployment, but retains stable
//! DOM markers for series titles and chapter rows.
use crate::{DOMAIN, parse_iso, status_from};
use aidoku::{
	Chapter, Manga, Result, Viewer,
	alloc::{String, Vec, string::ToString},
	imports::html::{Element, Html},
	prelude::*,
};

fn chapter_number(label: &str) -> Option<f32> {
	let digits: String = label
		.chars()
		.skip_while(|ch| !ch.is_ascii_digit())
		.take_while(|ch| ch.is_ascii_digit() || *ch == '.')
		.collect();
	digits.trim_matches('.').parse::<f32>().ok()
}

fn field(document: &aidoku::imports::html::Document, name: &str) -> Option<String> {
	let fields = document.select("dl dt")?;
	for field in fields {
		if field.text().as_deref() == Some(name) {
			return field.next().and_then(|element| element.text());
		}
	}
	None
}

fn metadata_image(document: &aidoku::imports::html::Document) -> Option<String> {
	if let Some(images) = document.select("script[type='application/ld+json']") {
		for node in images {
			let Some(data) = node.data() else { continue };
			let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else { continue };
			if let Some(image) = value.get("image").and_then(|item| item.as_str()) {
				return Some(image.to_string());
			}
		}
	}
	document.select_first("meta[property='og:image']")
		.and_then(|tag| tag.attr("content"))
}

fn parse_chapter_row(link: Element) -> Option<Chapter> {
	let url = link.attr("abs:href").or_else(|| link.attr("href"))?;
	let key = url.split("/comic/").nth(1)?
		.split('/').nth(1)?
		.split(['?', '#']).next()?
		.to_string();
	if key.is_empty() {
		return None;
	}
	// The whole anchor also contains a relative timestamp, e.g.
	// <span>Chapter 27</span><span><time>16 days ago</time></span>.
	// Parsing link.text() wrongly merges them into "Chapter 2716".
	// The chapter title is the span beside the one containing <time>.
	let chapter_name = link
		.select_first("div:has(> span > time) > span:not(:has(time))")
		.and_then(|element| element.text())
		.or_else(|| {
			// Alternative layouts still tend to render chapter names inside
			// their own span/heading. Never fall back to the combined row text.
			link.select("span:not(:has(time)), h3, h4, strong")
				.and_then(|elements| {
					elements
						.filter_map(|element| element.text())
						.find(|label| {
							let lower = label.trim().to_ascii_lowercase();
							lower.starts_with("chapter ")
								|| lower.starts_with("chapter-")
								|| lower.starts_with("episode ")
								|| lower.starts_with("ep. ")
						})
				})
		});
	let number = chapter_name.as_deref().and_then(chapter_number)
		.or_else(|| chapter_number(&key));
	let text = link.text().unwrap_or_default();
	let date_uploaded = link.select_first("time")
		.and_then(|tag| tag.attr("datetime"))
		.as_deref()
		.and_then(|date| parse_iso(Some(date)));
	let lower = text.to_ascii_lowercase();
	let locked = lower.contains("premium") || lower.contains("early access");
	Some(Chapter {
		key,
		url: Some(url),
		chapter_number: number,
		date_uploaded,
		locked,
		language: Some("en".into()),
		..Default::default()
	})
}

pub fn manga_update(
	mut manga: Manga,
	payload: &str,
	needs_details: bool,
	needs_chapters: bool,
) -> Result<Manga> {
	let document = Html::parse_with_url(payload, DOMAIN)
		.map_err(|_| error!("Temple Scan: invalid rendered series page"))?;
	if needs_details {
		manga.title = document
			.select_first("#series-title, h1")
			.and_then(|element| element.text())
			.ok_or_else(|| error!("Temple Scan: series title not found"))?;
		manga.cover = metadata_image(&document).or(manga.cover);
		manga.description = document.select_first("#series-synopsis-text")
			.and_then(|element| element.text());
		manga.authors = field(&document, "Author")
			.map(|name| aidoku::alloc::vec![name]);
		manga.artists = field(&document, "Studio")
			.map(|name| aidoku::alloc::vec![name]);
		manga.status = status_from(document
			.select_first("ul[aria-label='Series stats']")
			.and_then(|element| element.text()).as_deref());
		manga.url = Some(aidoku::alloc::format!("{DOMAIN}/comic/{}", manga.key));
		manga.viewer = Viewer::Webtoon;
	}
	if needs_chapters {
		let mut chapters = Vec::<Chapter>::new();
		if let Some(links) = document.select("a[id^='chapter-link-']") {
			for link in links {
				let Some(chapter) = parse_chapter_row(link) else { continue };
				if !chapters.iter().any(|previous| previous.key == chapter.key) {
					chapters.push(chapter);
				}
			}
		}
		if chapters.is_empty() {
			bail!("Temple Scan: no chapters in the rendered series page");
		}
		chapters.sort_by(|a, b| {
			b.chapter_number.partial_cmp(&a.chapter_number)
				.unwrap_or(core::cmp::Ordering::Equal)
		});
		manga.chapters = Some(chapters);
	}
	Ok(manga)
}
