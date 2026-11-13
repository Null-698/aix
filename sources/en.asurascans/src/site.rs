//! Load pages through the browser engine when native requests encounter Cloudflare.
//! Keep a WebView alive per source instance so adjacent chapter preloads do not
//! repeatedly create and bootstrap a new browser session.
use crate::BASE_URL;
use aidoku::{
	Result,
	alloc::{String, format, string::ToString},
	imports::{
		html::{Document, Html},
		js::WebView,
		net::Request,
	},
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
	document
		.select_first("title")
		.and_then(|el| el.text())
		.is_some_and(|title| {
			let title = title.to_ascii_lowercase();
			title.contains("just a moment") || title.contains("attention required")
		})
		|| document.select_first("#challenge-form, #cf-challenge-running").is_some()
}

fn native_html(url: &str) -> Result<(Document, String)> {
	let response = Request::get(url)?.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("AsuraScans: HTTP {status}");
	}
	let document = response.get_html()?;
	if is_challenge(&document) {
		bail!("AsuraScans: native request received a Cloudflare challenge");
	}
	Ok((document, response.get_url().unwrap_or_else(|| url.to_string())))
}

pub struct HtmlClient {
	view: WebView,
	initialized: bool,
}

impl HtmlClient {
	pub fn new() -> Self {
		Self {
			view: WebView::new(),
			initialized: false,
		}
	}

	fn browser_html(&mut self, url: &str) -> Result<(Document, String)> {
		if !self.initialized {
			// Set the same origin as the real website, then issue same-origin
			// browser requests with WebKit's cookie store.
			self.view
				.load_html_blocking(
					"<!doctype html><html><body></body></html>",
					Some(BASE_URL)
				)
				.map_err(|_| error!("AsuraScans: could not initialize browser networking"))?;
			self.initialized = true;
		}

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
		let reply = self.view.eval(&script)
			.map_err(|_| error!("AsuraScans: browser request failed"))?;
		let result: WebResponse = serde_json::from_str(&reply)
			.map_err(|_| error!("AsuraScans: invalid browser response"))?;
		if !(200..300).contains(&result.status) {
			bail!(
				"AsuraScans: browser HTTP {} ({})",
				result.status,
				result.error.unwrap_or_else(|| "request rejected".into())
			);
		}
		let document = Html::parse_with_url(&result.html, &result.url)
			.map_err(|_| error!("AsuraScans: invalid browser HTML"))?;
		if is_challenge(&document) {
			bail!("AsuraScans: browser still received a Cloudflare challenge");
		}
		Ok((document, result.url))
	}

	/// Manga details: try the standard network API first, with WebKit fallback.
	pub fn get_html(&mut self, url: &str) -> Result<(Document, String)> {
		native_html(url).or_else(|_| self.browser_html(url))
	}

	/// Chapter pages: WebKit first. In particular, don't delay Aidoku's
	/// next-chapter preload by waiting for a known failing native CF request.
	pub fn get_html_for_reading(&mut self, url: &str) -> Result<(Document, String)> {
		self.browser_html(url).or_else(|_| native_html(url))
	}
}
