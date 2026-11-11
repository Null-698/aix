//! Chapter access metadata stored in Astro component props.
//! HTML links do not reliably indicate premium and early-access locks.
use crate::helpers;
use aidoku::{
	alloc::{String, Vec, string::ToString},
	imports::{html::Document, std::{current_date, parse_date}},
};
use serde_json::Value;

pub struct ChapterMetadata {
	pub number: f32,
	pub title: Option<String>,
	pub date_uploaded: Option<i64>,
	/// None means the payload did not include lock information.
	pub locked: Option<bool>,
}

// Astro encodes primitive values as [0, value] and non-primitives as [1, value].
fn unwrap_astro(value: &Value) -> Value {
	match value {
		Value::Array(items) if items.len() == 2 && items[0].as_i64().is_some() => {
			unwrap_astro(&items[1])
		}
		Value::Array(items) if items.len() == 1 && items[0].as_i64().is_some() => {
			Value::Null
		}
		Value::Array(items) => Value::Array(items.iter().map(unwrap_astro).collect()),
		Value::Object(fields) => Value::Object(
			fields.iter().map(|(key, value)| (key.clone(), unwrap_astro(value))).collect()
		),
		_ => value.clone(),
	}
}

fn active_early_access(text: &str) -> Option<bool> {
	let timestamp = helpers::parse_chapter_date_text(text)
		.or_else(|| {
			let before_dot = text.split('.').next()?;
			parse_date(
				&aidoku::alloc::format!("{before_dot}Z"),
				"yyyy-MM-dd'T'HH:mm:ss'Z'",
			)
		})?;
	Some(timestamp > current_date())
}

fn entry_from_value(value: &Value) -> Option<ChapterMetadata> {
	let entry = value.as_object()?;
	let number = entry.get("number")?.as_f64()? as f32;
	if !number.is_finite() {
		return None;
	}
	let date_uploaded = entry.get("published_at")
		.or_else(|| entry.get("created_at"))
		.and_then(Value::as_str)
		.and_then(helpers::parse_chapter_date_text);
	let premium = entry.get("is_premium").and_then(Value::as_bool);
	let is_locked = entry.get("is_locked").and_then(Value::as_bool);
	let early_access = entry.get("early_access_until")
		.and_then(Value::as_str)
		.and_then(active_early_access);
	let locked = if premium == Some(true) || is_locked == Some(true) || early_access == Some(true) {
		Some(true)
	} else if premium.is_some() || is_locked.is_some() || early_access.is_some() {
		Some(false)
	} else {
		None
	};
	let title = entry.get("title").and_then(Value::as_str).map(ToString::to_string);
	Some(ChapterMetadata { number, title, date_uploaded, locked })
}

/// Search all Astro components, regardless of their version-specific names.
pub fn from_html(html: &Document) -> Vec<ChapterMetadata> {
	let mut result = Vec::<ChapterMetadata>::new();
	if let Some(islands) = html.select("astro-island[props]") {
		for island in islands {
			let Some(props) = island.attr("props") else { continue };
			if !props.contains("chapters") {
				continue;
			}
			let Ok(raw) = serde_json::from_str::<Value>(&props) else { continue };
			let decoded = unwrap_astro(&raw);
			let Some(chapters) = decoded.get("chapters").and_then(Value::as_array) else { continue };
			for chapter in chapters {
				let Some(row) = entry_from_value(chapter) else { continue };
				if let Some(existing) = result.iter_mut().find(|v| v.number == row.number) {
					*existing = row;
				} else {
					result.push(row);
				}
			}
		}
	}
	result
}
