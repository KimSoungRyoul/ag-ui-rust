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
			title: 'ag-ui-rust',
			description:
				'A Rust SDK for the AG-UI protocol — build agent backends and agent clients in Rust.',
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
			sidebar: [
				{
					label: 'Start here',
					items: [
						{ label: 'Getting started', link: '/start/' },
						{ label: 'How AG-UI works', link: '/start/protocol/' },
						{ label: 'The crates', link: '/start/crates/' },
					],
				},
				{
					label: 'Serving an agent',
					items: [
						{ label: 'The Agent trait', link: '/server/agent/' },
						{ label: 'Streaming text', link: '/server/text/' },
						{ label: 'Tool calls', link: '/server/tools/' },
						{ label: 'Shared state', link: '/server/state/' },
						{ label: 'Human in the loop', link: '/server/interrupts/' },
						{ label: 'Errors and cancellation', link: '/server/errors/' },
						{ label: 'Serving over HTTP', link: '/server/axum/' },
					],
				},
				{
					label: 'Consuming an agent',
					items: [
						{ label: 'Sessions', link: '/client/session/' },
						{ label: 'The update stream', link: '/client/updates/' },
						{ label: 'Rendering a run', link: '/client/rendering/' },
						{ label: 'Transports', link: '/client/transports/' },
					],
				},
				{
					label: 'A2UI',
					items: [
						{ label: 'Overview', link: '/a2ui/' },
						{ label: 'Authoring surfaces', link: '/a2ui/authoring/' },
						{ label: 'Validation', link: '/a2ui/validation/' },
					],
				},
				{
					label: 'Design',
					items: [
						{ label: 'Design commitments', link: '/design/commitments/' },
						{ label: 'Verification', link: '/design/verification/' },
						{ label: 'Testing', link: '/design/testing/' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Event reference', link: '/reference/events/' },
						{ label: 'Feature flags', link: '/reference/features/' },
						{ label: 'Platforms and MSRV', link: '/reference/platforms/' },
						{
							label: 'API docs (rustdoc)',
							// Starlight prefixes sidebar links with `base`, so this
							// resolves to /ag-ui-rust/api/ — the directory `cargo doc`
							// output is placed in at deploy time.
							link: '/api/',
							attrs: { target: '_blank' },
						},
					],
				},
				{
					label: 'Examples',
					items: [
						{ label: 'task-board (agent)', link: '/examples/task-board/' },
						{ label: 'board-watch (client)', link: '/examples/board-watch/' },
					],
				},
			],
		}),
	],
});
