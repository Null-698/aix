use crate::BASE_URL;
use aidoku::{
	alloc::string::String,
	imports::{html::Element, std::{current_date, parse_date}},
	prelude::*,
};

/// Returns the ID of a manga from a URL.
pub fn get_manga_key(url: &str) -> Option<String> {
	// Asura Scans appends a random string at the end of each series slug
	// The random string is not necessary, along with the trailing '-'

	// remove query parameters
	let path = url.split('?').next().unwrap_or("");

	// find the segment after "series"
	let manga_segment = path
		.split('/')
		.skip_while(|segment| *segment != "comics")
		.nth(1)?;

	// find the last '-' and keep it in the id
	let pos = manga_segment.rfind('-')?;
	Some(manga_segment[..pos].into())
}

/// Returns the ID of a chapter from a URL.
pub fn get_chapter_key(url: &str) -> Option<String> {
	// remove query parameters
	let path = url.split('?').next().unwrap_or("");

	// find the segment after "chapter"
	let chapter_segment = path
		.split('/')
		.skip_while(|segment| *segment != "chapter")
		.nth(1)?;

	// extract only the numeric (and '.') prefix
	let end_pos = chapter_segment
		.find(|c: char| !c.is_numeric() && c != '.')
		.unwrap_or(chapter_segment.len());

	Some(chapter_segment[..end_pos].into())
}

/// Returns full URL of a manga from a manga ID.
pub fn get_manga_url(manga_id: &str) -> String {
	format!("{BASE_URL}/comics/{manga_id}")
}

/// Returns full URL of a chapter from a chapter ID and manga ID.
pub fn get_chapter_url(chapter_id: &str, manga_id: &str) -> String {
	format!("{BASE_URL}/comics/{manga_id}/chapter/{chapter_id}")
}

/// Parse chapter dates shown by AsuraScans, including relative labels such as
/// "3 days ago" and "last week". Aidoku expects a Unix timestamp (seconds).
pub fn parse_chapter_date_text(text: &str) -> Option<i64> {
	let text = text.trim();
	if text.is_empty() {
		return None;
	}
	const FORMATS: &[&str] = &[
		"MMM dd, yyyy",
		"MMM d, yyyy",
		"MMMM d, yyyy",
		"yyyy-MM-dd",
		"yyyy-MM-dd'T'HH:mm:ss'Z'",
	];
	if let Some(date) = FORMATS
		.iter()
		.find_map(|format| parse_date(text, format))
	{
		return Some(date);
	}

	// Some pages retain an ISO timestamp with subsecond precision.
	if let Some((whole, fractional)) = text.split_once('.')
		&& whole.contains('T')
		&& fractional.ends_with('Z')
	{
		if let Some(date) = parse_date(
			&format!("{whole}Z"),
			"yyyy-MM-dd'T'HH:mm:ss'Z'",
		) {
			return Some(date);
		}
	}

	let lower = text.to_ascii_lowercase();
	let lower = lower.trim_end_matches('.');
	let now = current_date();
	const DAY: i64 = 24 * 60 * 60;
	match lower {
		"now" | "just now" | "today" | "a moment ago" | "moments ago" => return Some(now),
		"yesterday" => return Some(now - DAY),
		"last week" => return Some(now - 7 * DAY),
		"last month" => return Some(now - 30 * DAY),
		"last year" => return Some(now - 365 * DAY),
		_ => {}
	}
	let relative = lower.strip_suffix(" ago")?;
	let mut words = relative.split_whitespace();
	let count = words.next()?;
	let unit = words.next()?;
	if words.next().is_some() {
		return None;
	}
	let number = if count == "a" || count == "an" {
		1
	} else {
		count.parse::<i64>().ok()?
	};
	let seconds = match unit {
		"s" | "sec" | "secs" | "second" | "seconds" => 1,
		"m" | "min" | "mins" | "minute" | "minutes" => 60,
		"h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
		"d" | "day" | "days" => DAY,
		"w" | "week" | "weeks" => 7 * DAY,
		"month" | "months" => 30 * DAY,
		"year" | "years" => 365 * DAY,
		_ => return None,
	};
	Some(now.saturating_sub(number.saturating_mul(seconds)))
}

/// Prefer timestamp elements, then scan text children and the chapter label.
/// The latter handles markup that visually separates a chapter title and its date
/// without an identifying CSS class.
pub fn get_chapter_date(link: &Element) -> Option<i64> {
	if let Some(parts) = link.select("time, span") {
		for part in parts {
			if let Some(date) = part.attr("datetime")
				.and_then(|value| parse_chapter_date_text(&value))
			{
				return Some(date);
			}
			if let Some(date) = part.text()
				.and_then(|value| parse_chapter_date_text(&value))
			{
				return Some(date);
			}
		}
	}
	let label = link.text()?;
	for (index, _) in label.char_indices() {
		if let Some(date) = parse_chapter_date_text(&label[index..]) {
			return Some(date);
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn test_manga_keys() {
		assert_eq!(
			get_manga_key("https://asurascans.com/comics/swordmasters-youngest-son-cb22671f")
				.as_deref(),
			Some("swordmasters-youngest-son")
		);
		assert_eq!(
			get_manga_key(
				"https://asurascans.com/comics/swordmasters-youngest-son-cb22671f?blahblah"
			)
			.as_deref(),
			Some("swordmasters-youngest-son")
		);
		assert_eq!(
			get_manga_key(
				"https://asurascans.com/comics/swordmasters-youngest-son-cb22671f/chapter/1"
			)
			.as_deref(),
			Some("swordmasters-youngest-son")
		);
	}

	#[aidoku_test]
	fn test_chapter_keys() {
		assert_eq!(
			get_chapter_key("https://asurascans.com/comics/swordmasters-youngest-son-cb22671f"),
			None
		);
		assert_eq!(
			get_chapter_key(
				"https://asurascans.com/comics/swordmasters-youngest-son-cb22671f?blahblah"
			),
			None
		);
		assert_eq!(
			get_chapter_key(
				"https://asurascans.com/comics/swordmasters-youngest-son-cb22671f/chapter/1"
			)
			.as_deref(),
			Some("1")
		);
		assert_eq!(
			get_chapter_key("https://asurascans.com/comics/swordmasters-youngest-son/chapter/1")
				.as_deref(),
			Some("1")
		);
	}
}
