//! Fetch Asura HTML through the browser engine when native HTTP gets challenged.
//! A successful Cloudflare/browser session is not necessarily accepted by native HTTP.
use crate::BASE_URL;
use aidoku::{
	Result,
	alloc::{String, format, string::ToString},
	imports::{html::{Document, Html}, js::WebView, net::Request},
	prelude::*,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct WebResponse {
	status: i32,
	url: String,
	html: String,
	error: Option<String>,
}

fn is_challenge(document: &Document) -> bool {
	document.select_first("title").and_then(|el| el.text())
		.is_some_and(|title| {
			let title = title.to_ascii_lowercase();
			title.contains("just a moment") || title.contains("attention required")
		})
		|| document.select_first("#challenge-form, #cf-challenge-running").is_some()
}

/// Prefer the native transport (which can trigger Aidoku's challenge handler), then
/// retry through WebKit, which shares the visible browser's website data store.
pub fn get_html(url: &str) -> Result<(Document, String)> {
	let mut native_status = 0;
	if let Ok(request) = Request::get(url) {
		if let Ok(response) = request.send() {
			native_status = response.status_code();
			if (200..300).contains(&native_status) {
				if let Ok(document) = response.get_html() {
					if !is_challenge(&document) {
						return Ok((
							document,
							response.get_url().unwrap_or_else(|| url.to_string()),
						));
					}
				}
			}
		}
	}

	// load_html_blocking on a local stub avoids hanging on a challenged remote
	// navigation. The base URL establishes the site's origin for same-origin XHR.
	let view = WebView::new();
	view.load_html_blocking("<!doctype html><html><body></body></html>", Some(BASE_URL))
		.map_err(|_| error!("AsuraScans: could not initialize browser networking"))?;

	let target = serde_json::to_string(url)?;
	let script = format!(
		r#"(() => {{
			try {{
				const xhr = new XMLHttpRequest();
				xhr.open('GET', {target}, false);
				xhr.setRequestHeader('Accept', 'text/html,application/xhtml+xml');
				xhr.send();
				return JSON.stringify({{
					status: xhr.status,
					url: xhr.responseURL || {target},
					html: xhr.responseText,
					error: null
				}});
			}} catch (e) {{
				return JSON.stringify({{
					status: 0,
					url: {target},
					html: '',
					error: String(e)
				}});
			}}
		}})()"#
	);
	let reply = view.eval(&script)
		.map_err(|_| error!("AsuraScans: browser networking failed"))?;
	let result: WebResponse = serde_json::from_str(&reply)
		.map_err(|_| error!("AsuraScans: invalid browser response"))?;
	if !(200..300).contains(&result.status) {
		bail!(
			"AsuraScans: native HTTP {native_status}, browser HTTP {} ({})",
			result.status,
			result.error.unwrap_or_else(|| "request rejected".into())
		);
	}
	let document = Html::parse_with_url(&result.html, &result.url)
		.map_err(|_| error!("AsuraScans: invalid HTML from browser"))?;
	if is_challenge(&document) {
		bail!("AsuraScans: browser still received a Cloudflare challenge");
	}
	Ok((document, result.url))
}
