import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const website = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(website, 'dist');
const base = '/ag-ui-rust/';
const checkApi = process.argv.includes('--with-api');
const versions = ['v0.4.5', 'v0.5.0-alpha.1'];
const docsRoot = join(website, 'src/content/docs');

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
const sources = await files(docsRoot);
const pages = sources.filter((path) => /\.mdx?$/.test(path));
const snippetHarness = await readFile(join(website, '../e2e/src/website.rs'), 'utf8');
const pageSlugs = pages.map((source) => source.slice(docsRoot.length + 1));

// The 0.4.5 tree is an immutable snapshot from its release tag. The current
// doctest harness compiles the alpha guide against the current source tree.
for (const locale of ['', 'ko/']) {
  const paths = versions.map((version) =>
    pageSlugs
      .filter((slug) => slug.startsWith(`${locale}${version}/`))
      .map((slug) => slug.slice(`${locale}${version}/`.length))
      .sort()
  );
  assert.deepEqual(paths[0], paths[1], `Guide pages differ between versions: ${locale || 'en'}`);
  assert(paths[0].length > 0, `No versioned guide pages for ${locale || 'en'}`);
}

for (const source of pages) {
  const slug = source.slice(docsRoot.length + 1);
  const markdown = await readFile(source, 'utf8');
  if (/^draft: true$/m.test(markdown)) continue;
  if (!/^(?:ko\/)?v0\.4\.5\//.test(slug) && /^```rust\b/m.test(markdown)) {
    assert(snippetHarness.includes(`"${slug}"`), `Rust snippets missing from doctest harness: ${slug}`);
  }
  const route = slug.replace(/(?:\/index)?\.mdx?$/, '');
  const page = join(dist, route, 'index.html');
  const html = await readFile(page, 'utf8');
  const body = html.match(/<div[^>]*class="[^"]*\bsl-markdown-content\b[^"]*"[^>]*>([\s\S]*?)<\/div>/)?.[1];
  assert(body && body.replace(/<[^>]*>/g, '').trim().length > 20, `Empty document body: ${slug}`);
  assert(
    html.includes(`rel="canonical" href="https://kimsoungryoul.github.io${base}${route}/"`),
    `Wrong canonical URL: ${slug}`
  );
  if (/^(?:ko\/)?v0\.(?:4\.5|5\.0-alpha\.1)\//.test(route)) {
    assert(html.includes('aria-label="Documentation version"') || html.includes('aria-label="문서 버전"'),
      `Missing version navigation: ${slug}`);
  }
  for (const [, href] of html.matchAll(/<a\b[^>]*\bhref="([^"]+)"/g)) {
    if (!href.startsWith('#') && !href.startsWith(base)) continue;
    if (!checkApi && href.startsWith(`${base}api/`)) continue;

    let document = html;
    if (!href.startsWith('#')) {
      const pathname = decodeURIComponent(href.split(/[?#]/)[0].slice(base.length));
      const target = join(dist, pathname);
      const info = await stat(target).catch(() => null);
      assert(info, `Broken navigation in ${slug}: ${href}`);
      const targetFile = info.isDirectory() ? join(target, 'index.html') : target;
      if (info.isDirectory()) await stat(targetFile);
      if (href.includes('#')) document = await readFile(targetFile, 'utf8');
    }

    const fragment = href.split('#')[1];
    if (fragment) {
      const anchor = decodeURIComponent(fragment);
      assert(document.includes(`id="${anchor}"`), `Broken anchor in ${slug}: ${href}`);
    }
  }
}
for (const locale of ['', 'ko/']) {
  const html = await readFile(join(dist, locale, 'index.html'), 'utf8');
  assert(html.includes(`content="0;url=${base}${locale}versions/"`), `Wrong home redirect: ${locale || 'root'}`);

  for (const slug of pageSlugs.filter((slug) => slug.startsWith(`${locale}v0.5.0-alpha.1/`))) {
    const route = slug.slice(`${locale}v0.5.0-alpha.1/`.length).replace(/(?:\/index)?\.mdx?$/, '');
    const redirect = await readFile(join(dist, locale, route, 'index.html'), 'utf8');
    assert(
      redirect.includes(`content="0;url=${base}${locale}v0.5.0-alpha.1/${route}/"`),
      `Wrong legacy redirect: ${locale}${route}`
    );
  }
}
console.log(`Verified ${pages.length} document bodies, version parity, navigation, snippets and redirects.`);
