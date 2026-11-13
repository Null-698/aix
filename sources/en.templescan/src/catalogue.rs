//! Drive TempleToons' actual React catalogue controls in WebKit.
//! The server initially renders only a single status/order and a handful of
//! cards. Sorting that HTML locally makes every tab appear to be "Latest".
use crate::{BrowseSeries, network::BrowserClient};
use aidoku::{
	Result,
	alloc::{String, Vec, format},
	imports::std::sleep,
	prelude::*,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct CatalogueStep {
	state: String,
	#[serde(default)]
	entries: Vec<BrowseSeries>,
	#[serde(default)]
	has_more: bool,
	error: Option<String>,
}

const CATALOGUE_SCRIPT: &str = r#"(args => {
    const encode = (state, entries = [], has_more = false, error = null) =>
        JSON.stringify({state, entries, has_more, error});
    const pending = () => encode('wait');
    const fail = error => encode('error', [], false, error);
    if (location.pathname !== '/comics') return fail('Temple Scan did not open the comics catalogue');

    const state = window.__aidokuTempleCatalogue ||
        (window.__aidokuTempleCatalogue = {started: Date.now(), changed: 0, moreAt: 0, lastCount: 0, typed: false});

    if (Date.now() - state.started > 60000)
        return fail('Temple Scan catalogue did not finish loading within 60 seconds');

    const buttons = Array.from(document.querySelectorAll('button'));
    const button = test => buttons.find(b => test.test((b.textContent || '').trim()));
    const slugOf = link => {
        try {
            return new URL(link.href, location.origin).pathname.split('/comic/')[1]?.split('/')[0] || '';
        } catch (_) {
            return '';
        }
    };
    const item = (link, label, image, status) => ({
        series_slug: slugOf(link),
        title: (label || '').trim(),
        thumbnail: image || null,
        status: status || null
    });

    if (args.query) {
        let input = Array.from(document.querySelectorAll('input[type=search]'))
            .find(el => el.getClientRects().length > 0);
        if (!input) {
            if (!state.searchOpened) {
                const openButton = buttons.find(b =>
                    /search/i.test(b.getAttribute('aria-label') || '') ||
                    !!b.querySelector('svg.lucide-search')
                );
                if (openButton) {
                    openButton.click();
                    state.searchOpened = true;
                }
            }
            return pending();
        }

        if (!state.typed || input.value !== args.query) {
            const nativeSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
            nativeSetter?.call(input, args.query);
            input.dispatchEvent(new Event('input', {bubbles:true}));
            state.typed = true;
            state.changed = Date.now();
            return pending();
        }
        if (Date.now() - state.changed < 500) return pending();
        const text = document.body?.innerText || '';
        if (/No results for/i.test(text)) return encode('done');
        if (/Search unavailable/i.test(text)) return fail('Temple Scan website search is unavailable');

        const list = document.querySelector('[role=listbox]');
        if (!list) return pending();
        const options = Array.from(list.querySelectorAll('[role=option][href*="/comic/"]'));
        if (options.length < args.needed) {
            const showMore = button(/^show more results$/i);
            if (showMore) {
                if (options.length !== state.lastCount || Date.now() - state.moreAt > 2500) {
                    state.lastCount = options.length;
                    state.moreAt = Date.now();
                    showMore.click();
                }
                return pending();
            }
        }
        const entries = options.map(option => {
            const label = option.querySelector('span:last-child > span')?.textContent?.trim() ||
                option.querySelector('span')?.textContent?.trim() ||
                option.textContent?.trim() || '';
            const cover = option.querySelector('img')?.src || null;
            const tags = Array.from(option.querySelectorAll('span:last-child > span'))
                .map(x => x.textContent?.trim() || '');
            return item(option, label, cover, tags[1]);
        }).filter(e => e.series_slug && e.title);
        const more = !!button(/^show more results$/i) ||
            /Showing \d+ of \d+/.test(text) && (() => {
                const found = /Showing (\d+) of (\d+)/.exec(text);
                return found && Number(found[1]) < Number(found[2]);
            })();
        return encode('done', entries, more);
    }

    const selects = Array.from(document.querySelectorAll('select'));
    if (!selects.length || !selects.every(s =>
        Object.keys(s).some(key => key.startsWith('__reactProps$')))) return pending();

    for (const value of [args.status, args.order, 'descendente']) {
        const select = selects.find(s => Array.from(s.options).some(o => o.value === value));
        if (!select) return fail('Temple Scan website no longer has the filter ' + value);
        if (select.value !== value) {
            const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
            setter?.call(select, value);
            select.dispatchEvent(new Event('change', {bubbles:true}));
            state.changed = Date.now();
            state.moreAt = 0;
            state.lastCount = 0;
            return pending();
        }
    }

    // Wait until React has processed the last change; otherwise we would read
    // the old cards while the website is fetching the newly selected sort.
    if (state.changed && Date.now() - state.changed < 750) return pending();

    const links = Array.from(document.querySelectorAll('main a[href^="/comic/"]'))
        .filter(a => a.querySelector('h2'));
    const entries = links.map(link => {
        const card = link.parentElement;
        const img = card?.querySelector('img');
        const label = link.querySelector('h2')?.textContent?.trim();
        return item(link, label, img?.currentSrc || img?.src || null, args.status);
    }).filter(e => e.series_slug && e.title);
    const unique = Array.from(new Map(entries.map(e => [e.series_slug, e])).values());
    const loadMore = button(/^load more$/i);

    if (unique.length < args.needed && loadMore) {
        if (state.lastCount !== unique.length || Date.now() - state.moreAt > 2500) {
            state.lastCount = unique.length;
            state.moreAt = Date.now();
            loadMore.click();
        }
        return pending();
    }
    // The site may have no series for this status.
    if (!unique.length && Date.now() - state.started < 2200) return pending();
    return encode('done', unique, !!loadMore);
})"#;

impl BrowserClient {
	pub fn browse_catalogue(
		&mut self,
		query: &str,
		status: &str,
		order: &str,
		needed: usize,
	) -> Result<(Vec<BrowseSeries>, bool)> {
		self.get_html("https://templetoons.com/comics")?;
		let args = serde_json::json!({
			"query": query,
			"status": status,
			"order": order,
			"needed": needed,
		});
		let script = format!("({CATALOGUE_SCRIPT})({args})");
		for _ in 0..65 {
			let json = self.web_view.eval(&script)
				.map_err(|_| error!("Temple Scan could not interact with the website catalogue"))?;
			let step: CatalogueStep = serde_json::from_str(&json)
				.map_err(|_| error!("Temple Scan received an invalid catalogue response"))?;
			match step.state.as_str() {
				"done" => return Ok((step.entries, step.has_more)),
				"error" => bail!(
					"{}",
					step.error.unwrap_or_else(|| "Temple Scan catalogue failed".into())
				),
				"wait" => sleep(1),
				_ => bail!("Temple Scan received an unexpected catalogue state"),
			}
		}
		bail!("Temple Scan catalogue timed out; the website did not load its search or filters")
	}
}
