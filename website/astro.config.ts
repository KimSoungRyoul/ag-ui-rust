import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';
import starlightLinksValidator from 'starlight-links-validator';

// GitHub Pages serves this as a *project* site, so the deployed URL has the
// repository name in it. `site` and `base` are both needed and do different
// jobs: `site` is what makes canonical URLs and the sitemap absolute, `base` is
// what keeps every generated href inside /ag-ui-rust/ instead of pointing at
// the domain root, where nothing of ours is served.
//
// Neither Astro nor Starlight rewrites hand-written links: `<a href>`, MDX
// `<LinkCard href>` and the hero actions in frontmatter are emitted verbatim.
// Only links Starlight itself builds — sidebar entries, the favicon, prev/next
// — get `base` prefixed for free. Everything hand-written has to carry the base
// in the page, and it is written out rather than interpolated from
// `import.meta.env.BASE_URL`, because the links validator below only reads
// string literals and would skip an interpolated href without saying so. A
// wrong base is then a red build instead of a 404 after deploy.
const site = 'https://kimsoungryoul.github.io';
const base = '/ag-ui-rust';

export default defineConfig({
	site,
	base,
	integrations: [
		starlight({
			// Per-language site titles are supported — Starlight's schema types
			// `title` as `string | Record<lang, string>` and requires a key for the
			// default language. It is left as a plain string anyway, because the
			// title is the crate name and a crate name is spelled the same in
			// Korean. The record form could only ever hold a duplicate of this
			// line, and a duplicate is one more place to forget when a name
			// changes.
			title: 'ag-ui-rust',
			// `description`, unlike `title`, has no per-locale form at all: the
			// schema is a plain optional string, and Starlight uses it only as the
			// fallback for a page that sets none (`data.description ||
			// config.description`). That costs nothing here, because every page
			// carries its own `description` in frontmatter, so a Korean page gets a
			// Korean description from the page itself rather than from this line.
			description:
				'A Rust SDK for the AG-UI protocol — build agent backends and agent clients in Rust.',
			// `root` is the locale key that means "served from the site root", so
			// English stays at /ag-ui-rust/start/ rather than moving to
			// /ag-ui-rust/en/start/. That is the whole reason for spelling English
			// as `root` instead of `en`: adding a second language must not change
			// a single existing URL, and this was checked rather than assumed — the
			// set of HTML files under dist/ is identical before and after, with
			// /ko/* the only addition.
			//
			// Korean pages that do not exist yet are not 404s. Starlight builds a
			// fallback route for every default-locale page a translation is missing
			// for, serving the English content under /ko/ with a notice above it,
			// so the Korean sidebar is fully navigable from the first page
			// translated rather than the last.
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ko: { label: '한국어', lang: 'ko' },
			},
			// Nothing in this repo supplies Korean UI strings, and nothing needs
			// to: Starlight ships translations/ko.json in the package, covering the
			// search box, the theme and language pickers, "on this page", prev/next,
			// the aside titles and the untranslated-content notice. A
			// src/content/i18n/ collection is therefore deliberately absent — an
			// override file would only be a copy of the package's, and copies rot.
			logo: {
				src: './src/assets/logo.svg',
				alt: 'ag-ui-rust',
			},
			// Starlight prefixes this with `base` itself (`fileWithBase`), so the
			// path here is site-root-relative and must not repeat /ag-ui-rust.
			favicon: '/favicon.svg',
			social: [
				{
					icon: 'github',
					label: 'GitHub',
					href: 'https://github.com/KimSoungRyoul/ag-ui-rust',
				},
			],
			editLink: {
				baseUrl: 'https://github.com/KimSoungRyoul/ag-ui-rust/edit/main/website/',
			},
			// Read from `git log` at build time. The deploy workflow therefore needs
			// real history — a `fetch-depth: 1` checkout would date every page to
			// the day of the deploy.
			lastUpdated: true,
			customCss: ['./src/styles/custom.css'],
			plugins: [
				starlightLinksValidator({
					// The point of installing this at all: a broken internal link is a
					// red build, not a warning nobody reads. Explicit rather than left
					// to the default, because the value is what the CI gate depends on.
					failOnError: true,
					// A Korean page linking to a page that has not been translated
					// yet is not an error. Starlight serves the English text under
					// its own "not yet translated" notice at that URL, which is a
					// working page and a deliberate feature; left at its default of
					// `true`, this plugin rejects the link anyway, and the whole
					// site then has to be translated atomically or not linked
					// across at all. Every new English page would owe a Korean one
					// before anything Korean could point at it, which is the kind
					// of tax that ends with someone deleting the check.
					//
					// Verified this does not blunt the gate: with it off, a link to
					// a page that exists in neither locale still fails, and so does
					// a link to a heading anchor that does not exist. What it
					// accepts is exactly the fallback case.
					errorOnFallbackPages: false,
					// The rustdoc at /ag-ui-rust/api/ is injected at deploy time rather
					// than built by Astro, so to this plugin the whole directory is
					// simply missing and any prose link into it is an error. The
					// sidebar entry below is not what needs this — sidebar links are
					// not validated at all — the pages that cite a type are.
					// Patterns are matched against the link exactly as authored, which
					// is why they carry the base.
					exclude: [`${base}/api`, `${base}/api/**`],
				}),
			],
			// Every label carries its Korean translation inline. Starlight keys
			// `translations` by language tag rather than by locale key and falls
			// back to `label` when a language is missing, so an untranslated entry
			// is silently English rather than an error — which is exactly why they
			// are filled in here in one pass instead of page by page.
			sidebar: [
				{
					label: 'Start here',
					translations: { ko: '시작하기' },
					items: [
						{ label: 'Getting started', translations: { ko: '시작하기' }, link: '/start/' },
						{
							label: 'How AG-UI works',
							translations: { ko: 'AG-UI 동작 방식' },
							link: '/start/protocol/',
						},
						{ label: 'The crates', translations: { ko: 'crate 구성' }, link: '/start/crates/' },
					],
				},
				{
					label: 'Serving an agent',
					translations: { ko: 'agent serving' },
					items: [
						{ label: 'The Agent trait', translations: { ko: 'Agent trait' }, link: '/server/agent/' },
						{ label: 'Streaming text', translations: { ko: 'text streaming' }, link: '/server/text/' },
						{ label: 'Tool calls', translations: { ko: 'tool call' }, link: '/server/tools/' },
						{ label: 'Shared state', translations: { ko: 'shared state' }, link: '/server/state/' },
						{
							label: 'Human in the loop',
							translations: { ko: 'human in the loop' },
							link: '/server/interrupts/',
						},
						{
							label: 'Errors and cancellation',
							translations: { ko: 'error와 cancellation' },
							link: '/server/errors/',
						},
						{ label: 'Serving over HTTP', translations: { ko: 'HTTP로 serving' }, link: '/server/axum/' },
					],
				},
				{
					label: 'Consuming an agent',
					translations: { ko: 'agent 사용' },
					items: [
						{ label: 'Sessions', translations: { ko: 'session' }, link: '/client/session/' },
						{
							label: 'The update stream',
							translations: { ko: 'update stream' },
							link: '/client/updates/',
						},
						{
							label: 'Rendering a run',
							translations: { ko: 'run rendering' },
							link: '/client/rendering/',
						},
						{ label: 'Transports', translations: { ko: 'transport' }, link: '/client/transports/' },
					],
				},
				{
					label: 'A2UI',
					translations: { ko: 'A2UI' },
					items: [
						{ label: 'Overview', translations: { ko: '개요' }, link: '/a2ui/' },
						{ label: 'Authoring surfaces', translations: { ko: 'surface 작성' }, link: '/a2ui/authoring/' },
						{ label: 'Validation', translations: { ko: 'validation' }, link: '/a2ui/validation/' },
					],
				},
				{
					label: 'Design',
					translations: { ko: '설계' },
					items: [
						{
							label: 'Design commitments',
							translations: { ko: '설계 원칙' },
							link: '/design/commitments/',
						},
						{ label: 'Verification', translations: { ko: 'verification' }, link: '/design/verification/' },
						{ label: 'Testing', translations: { ko: 'testing' }, link: '/design/testing/' },
					],
				},
				{
					label: 'Reference',
					translations: { ko: 'reference' },
					items: [
						{
							label: 'Event reference',
							translations: { ko: 'event reference' },
							link: '/reference/events/',
						},
						{ label: 'Feature flags', translations: { ko: 'feature flag' }, link: '/reference/features/' },
						{
							label: 'Platforms and MSRV',
							translations: { ko: 'platform과 MSRV' },
							link: '/reference/platforms/',
						},
						{
							label: 'API docs (rustdoc)',
							translations: { ko: 'API 문서 (rustdoc)' },
							// Written out as a full URL, and that is load-bearing rather
							// than sloppy. Starlight injects the current locale into every
							// relative sidebar link — a bare `/api/` becomes `/ko/api/` on
							// Korean pages — and the rustdoc is copied to /ag-ui-rust/api/
							// once, not once per language. A link with a protocol skips
							// both the locale injection and the `base` prefixing and is
							// emitted verbatim, which is the only form that lands on the
							// same rustdoc from both languages. Nothing catches this if it
							// regresses: sidebar links are not validated.
							link: `${site}${base}/api/`,
							attrs: { target: '_blank' },
						},
					],
				},
				{
					label: 'Examples',
					translations: { ko: '예제' },
					items: [
						{
							label: 'task-board (agent)',
							translations: { ko: 'task-board (agent)' },
							link: '/examples/task-board/',
						},
						{
							label: 'board-watch (client)',
							translations: { ko: 'board-watch (client)' },
							link: '/examples/board-watch/',
						},
					],
				},
			],
		}),
	],
});
