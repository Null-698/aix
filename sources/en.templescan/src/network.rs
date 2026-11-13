//! Browser-backed Next.js RSC requests for TempleToons.
//! Native HTTP can be redirected to the site's Cloudflare Turnstile page.
use crate::DOMAIN;
use aidoku::{
	Result,
	alloc::{String, format, string::ToString},
	imports::{js::WebView, net::Request},
	prelude::*,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct BrowserResponse {
	status: i32,
	url: String,
	body: String,
	error: Option<String>,
}

fn is_verification_page(url: &str, body: &str) -> bool {
	url.contains("/challenge")
		|| body.contains("Templetoons uses Cloudflare Turnstile")
		|| body.contains("cf-turnstile")
}

fn validate(url: &str, body: String, status: i32) -> Result<String> {
	if is_verification_page(url, &body) {
		bail!("Temple Scan needs website verification. Open templetoons.com in Aidoku's browser, complete its check, then refresh the source.");
	}
	if !(200..300).contains(&status) {
		bail!("Temple Scan returned HTTP {status}");
	}
	if body.trim().is_empty() {
		bail!("Temple Scan returned an empty response");
	}
	Ok(body)
}

pub struct BrowserClient {
	web_view: WebView,
	initialized: bool,
}

impl BrowserClient {
	pub fn new() -> Self {
		Self { web_view: WebView::new(), initialized: false }
	}

	fn browser_rsc(&mut self, url: &str) -> Result<String> {
		if !self.initialized {
			self.web_view
				.load_html_blocking("<!doctype html><html><body></body></html>", Some(DOMAIN))
				.map_err(|_| error!("Temple Scan could not initialize WebKit"))?;
			self.initialized = true;
		}
		let target = serde_json::to_string(url)?;
		let script = format!(
			r#"(() => {{
				try {{
					const xhr = new XMLHttpRequest();
					xhr.open('GET', {target}, false);
					xhr.setRequestHeader('RSC', '1');
					xhr.setRequestHeader('Accept', 'text/x-component');
					xhr.send();
					return JSON.stringify({{
						status: xhr.status,
						url: xhr.responseURL || {target},
						body: xhr.responseText,
						error: null
					}});
				}} catch (e) {{
					return JSON.stringify({{
						status: 0,
						url: {target},
						body: '',
						error: String(e)
					}});
				}}
			}})()"#
		);
		let reply = self.web_view.eval(&script)
			.map_err(|_| error!("Temple Scan could not make a WebKit request"))?;
		let response: BrowserResponse = serde_json::from_str(&reply)
			.map_err(|_| error!("Temple Scan received an invalid browser response"))?;
		if response.status == 0 {
			bail!(
				"Temple Scan WebKit request failed: {}",
				response.error.unwrap_or_else(|| "request did not complete".into())
			);
		}
		validate(&response.url, response.body, response.status)
	}

	fn native_rsc(url: &str) -> Result<String> {
		let response = Request::get(url)?
			.header("RSC", "1")
			.header("Accept", "text/x-component")
			.header("Referer", &format!("{DOMAIN}/"))
			.send()?;
		let status = response.status_code();
		let resolved = response.get_url().unwrap_or_else(|| url.to_string());
		validate(&resolved, response.get_string()?, status)
	}

	pub fn get_rsc(&mut self, url: &str) -> Result<String> {
		match self.browser_rsc(url) {
			Ok(body) => Ok(body),
			Err(web_error) => match Self::native_rsc(url) {
				Ok(body) => Ok(body),
				Err(_) => Err(web_error),
			}
		}
	}
}
