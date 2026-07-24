import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { JSDOM } from 'jsdom';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PATCH_SOURCE = readFileSync(
  path.join(__dirname, '../../assets/web-patches.js'),
  'utf8',
);

/**
 * Loads the full web-patches.js source into a fresh jsdom window and
 * executes it, exactly as the server injects it into every served HTML page.
 */
export function loadPatchedDom(bodyHtml = '') {
  const dom = new JSDOM(`<!doctype html><html><body>${bodyHtml}</body></html>`, {
    url: 'https://example.test/web/index.html',
    runScripts: 'dangerously',
  });
  const script = dom.window.document.createElement('script');
  script.textContent = PATCH_SOURCE;
  dom.window.document.body.appendChild(script);
  return dom;
}

/** Lets pending MutationObserver microtasks flush before assertions. */
export function nextTick() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
