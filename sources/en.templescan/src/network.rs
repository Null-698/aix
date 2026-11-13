//! Load TempleToons through normal WebKit page navigations.
//! The site rejects custom RSC/XHR requests even when normal page navigation
//! works in a browser; Next.js data can be read from the rendered document.
use crate::DOMAIN;
use aidoku::{
	Result,
	alloc::{String, string::ToString},
	imports::{js::WebView, net::Request},
	prelude::*,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct BrowserPage {
	url: String,
	body: String,
	status: i32,
}

fn is_verification_page(url: &str, body: &str) -> bool {
	url.contains("/challenge")
		|| body.contains("Templetoons uses Cloudflare Turnstile")
		|| body.contains("_cf_chl_opt")
		|| body.contains("cf-turnstile")
}

pub struct BrowserClient {
	pub(crate) web_view: WebView,
}

impl BrowserClient {
	pub fn new() -> Self {
		Self { web_view: WebView::new() }
	}

	/// Load a real page; do not send the custom RSC and Accept headers that
	/// cause 403 responses on TempleToons.
	pub fn get_html(&mut self, url: &str) -> Result<String> {
		self.web_view
			.load_blocking(Request::get(url)?)
			.map_err(|_| error!("Temple Scan: website navigation failed"))?;

		let reply = self.web_view.eval(
			r#"JSON.stringify({
				url: window.location.href,
				body: document.documentElement ? document.documentElement.outerHTML : '',
				status: performance.getEntriesByType('navigation')[0]?.responseStatus || 0
			})"#
		).map_err(|_| error!("Temple Scan: could not read the website page"))?;
		let page: BrowserPage = serde_json::from_str(&reply)
			.map_err(|_| error!("Temple Scan: invalid browser page"))?;

		if is_verification_page(&page.url, &page.body) {
			bail!("Temple Scan requires website verification. Open templetoons.com in Aidoku's source browser, complete its check, then refresh the source.");
		}
		if page.status >= 400 {
			bail!("Temple Scan: website returned HTTP {} in WebKit", page.status);
		}
		if page.body.trim().is_empty() {
			bail!("Temple Scan: website returned an empty HTML page");
		}
		Ok(page.body)
	}

	/// The current site may return opaque RSC names in HTML. The caller can
	/// parse either those embedded rows or the rendered HTML document.
	pub fn get_rsc(&mut self, url: &str) -> Result<String> {
		self.get_html(url)
	}

	/// The reader renders image elements lazily. Scroll to each missing image
	/// and collect its source once the website mounts it.
	pub fn get_reader_pages(&mut self, url: &str) -> Result<aidoku::alloc::Vec<String>> {
		let _ = self.get_html(url)?;
		let mut best = aidoku::alloc::Vec::<String>::new();
		for _ in 0..55 {
			let script = r#"(() => {
				if (!window.__templePages) {
					window.__templePages = {};
				}
				const pages = window.__templePages;
				const read = img => {
					const box = img.closest('[data-index]');
					if (box && (img.currentSrc || img.src)) {
						const url = img.currentSrc || img.src;
						if (url.startsWith('http')) pages[box.dataset.index] = url;
					}
				};
				document.querySelectorAll('[data-index] img').forEach(read);
				Array.from(document.querySelectorAll('button')).find(b =>
					b.textContent.trim() === 'Yes, I am an adult'
				)?.click();
				document.documentElement.style.zoom = '0.01';
				const boxes = Array.from(document.querySelectorAll('[data-index]'));
				const missing = boxes.find(box => !pages[box.dataset.index]);
				if (missing) missing.scrollIntoView({block:'center'});
				return JSON.stringify({
					locked: document.body && document.body.innerText.includes('This chapter is early access'),
					complete: boxes.length > 0 && !missing,
					urls: boxes.map(box => pages[box.dataset.index] || '')
				});
			})()"#;
			let response = self.web_view.eval(script)
				.map_err(|_| error!("Temple Scan: unable to inspect reader images"))?;
			let state: ReaderState = serde_json::from_str(&response)
				.map_err(|_| error!("Temple Scan: invalid reader response"))?;
			if state.locked {
				bail!("Temple Scan: this chapter requires a subscription");
			}
			let pages: aidoku::alloc::Vec<String> = state.urls.into_iter()
				.filter(|url| !url.is_empty())
				.collect();
			if pages.len() > best.len() {
				best = pages;
			}
			if state.complete {
				return Ok(best);
			}
		}
		if best.is_empty() {
			bail!("Temple Scan: reader did not load page images");
		}
		Ok(best)
	}
}

#[derive(Deserialize)]
struct ReaderState {
	locked: bool,
	complete: bool,
	urls: aidoku::alloc::Vec<String>,
}
