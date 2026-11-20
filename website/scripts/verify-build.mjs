import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { guideHref, guideVersions, hasGuidePage, publishedVersion, readGuideRoute } from '../src/guideVersions.mjs';

const website = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(website, 'dist');
const base = '/ag-ui-rust/';
const checkApi = process.argv.includes('--with-api');
const versions = guideVersions.map(({ id }) => id);
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

// Each guide has one page set in both locales. Different versions may have
// different guides, but the version switcher still needs a valid destination.
const routesByVersion = new Map();
for (const version of versions) {
  const routes = ['', 'ko/'].map((locale) =>
    pageSlugs
      .filter((slug) => slug.startsWith(`${locale}${version}/`))
      .map((slug) => slug.slice(`${locale}${version}/`.length).replace(/(?:\/index)?\.mdx?$/, '/'))
      .sort()
  );
  assert.deepEqual(routes[0], routes[1], `English and Korean guides differ for ${version}`);
  assert(routes[0].includes('start/'), `No start page for ${version}`);
  routesByVersion.set(version, routes[0]);
}
for (const version of versions) {
  for (const page of routesByVersion.get(publishedVersion)) {
    assert.equal(
      hasGuidePage(version, page), routesByVersion.get(version).includes(page),
      `Version switcher page list differs from the guides: ${version}/${page}`
    );
  }
}

for (const source of pages) {
  const slug = source.slice(docsRoot.length + 1);
  const markdown = await readFile(source, 'utf8');
  if (/^draft: true$/m.test(markdown)) continue;
  if (readGuideRoute(slug)?.version.id === publishedVersion && /^\s*```(?:rust|compile_fail)\b/m.test(markdown)) {
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
  const guide = readGuideRoute(route);
  if (guide) {
    assert(html.includes('aria-label="Documentation version"') || html.includes('aria-label="문서 버전"'),
      `Missing version navigation: ${slug}`);
    const selectedVersion = guide.version.id;
    const locale = route.startsWith('ko/') ? 'ko/' : '';
    for (const version of versions) {
      const href = guideHref(base.slice(0, -1), locale, version, guide.page);
      assert(html.split(`href="${href}"`).length >= 3, `Missing desktop or mobile version destination in ${slug}: ${href}`);
    }
    const sidebar = html.slice(html.indexOf('id="starlight__sidebar"'), html.indexOf('</sl-sidebar-state-persist>'));
    assert(sidebar.includes(`href="${base}${locale}${selectedVersion}/start/"`),
      `Missing current version in sidebar: ${slug}`);
    assert(sidebar.includes(`href="${base}${route}/"`), `Guide missing from sidebar: ${slug}`);
    for (const otherVersion of versions.filter((version) => version !== selectedVersion)) {
      assert(!sidebar.includes(`/${otherVersion}/`), `Other version leaked into sidebar: ${slug}`);
    }
  }
  if (/^(?:ko\/)?v0\.5\.0-alpha\.1\//.test(route)) {
    assert(html.includes('data-superseded-version="0.5.0-alpha.1"'),
      `Missing alpha.1 superseded notice: ${slug}`);
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
  assert(html.includes(`content="0;url=${base}${locale}${publishedVersion}/start/"`), `Wrong home redirect: ${locale || 'root'}`);

  for (const slug of pageSlugs.filter((slug) => slug.startsWith(`${locale}${publishedVersion}/`))) {
    const route = slug.slice(`${locale}${publishedVersion}/`.length).replace(/(?:\/index)?\.mdx?$/, '');
    const redirect = await readFile(join(dist, locale, route, 'index.html'), 'utf8');
    assert(
      redirect.includes(`content="0;url=${base}${locale}${publishedVersion}/${route}/"`),
      `Wrong legacy redirect: ${locale}${route}`
    );
  }
}
console.log(`Verified ${pages.length} document bodies, translation parity, navigation, snippets and redirects.`);
