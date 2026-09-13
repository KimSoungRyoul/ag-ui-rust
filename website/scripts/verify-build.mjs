import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const website = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(website, 'dist');
const base = '/ag-ui-rust/';
const checkApi = process.argv.includes('--with-api');

async function files(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? files(path) : [path];
  }));
  return nested.flat();
}

// Astro's content loader can log a rendering error yet exit successfully with an empty page.
// Validate the artifact, including sidebar links the Markdown link validator does not check.
const sources = await files(join(website, 'src/content/docs'));
const pages = sources.filter((path) => /\.mdx?$/.test(path));
const snippetHarness = await readFile(join(website, '../e2e/src/website.rs'), 'utf8');
for (const source of pages) {
  const slug = source.slice(join(website, 'src/content/docs').length + 1);
  const markdown = await readFile(source, 'utf8');
  if (/^draft: true$/m.test(markdown)) continue;
  if (/^```rust\b/m.test(markdown)) {
    assert(snippetHarness.includes(`"${slug}"`), `Rust snippets missing from doctest harness: ${slug}`);
  }
  const route = slug.replace(/(?:\/index)?\.mdx?$/, '');
  const page = join(dist, route, 'index.html');
  const html = await readFile(page, 'utf8');
  const body = html.match(/<div[^>]*class="[^"]*\bsl-markdown-content\b[^"]*"[^>]*>([\s\S]*?)<\/div>/)?.[1];
  assert(body && body.replace(/<[^>]*>/g, '').trim().length > 20, `Empty document body: ${slug}`);
  for (const [, href] of html.matchAll(/<a\b[^>]*\bhref="([^"]+)"/g)) {
    if (!href.startsWith(base) || (!checkApi && href.startsWith(`${base}api/`))) continue;
    const pathname = decodeURIComponent(href.split(/[?#]/)[0].slice(base.length));
    const target = join(dist, pathname);
    const info = await stat(target).catch(() => null);
    assert(info, `Broken navigation in ${slug}: ${href}`);
    if (info.isDirectory()) await stat(join(target, 'index.html'));
    if (checkApi && href.startsWith(`${base}api/`) && href.includes('#')) {
      const anchor = decodeURIComponent(href.split('#')[1]);
      const document = await readFile(info.isDirectory() ? join(target, 'index.html') : target, 'utf8');
      assert(document.includes(`id="${anchor}"`), `Broken API anchor in ${slug}: ${href}`);
    }
  }
}
for (const locale of ['', 'ko/']) {
  const html = await readFile(join(dist, locale, 'index.html'), 'utf8');
  assert(html.includes(`content="0;url=${base}${locale}start/"`), `Wrong home redirect: ${locale || 'root'}`);
}
console.log(`Verified ${pages.length} document bodies, navigation, snippet coverage and home redirects.`);
