import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';
import starlightLinksValidator from 'starlight-links-validator';
import { guideVersions, guideSidebarLabel, hasGuidePage, publishedVersion, readGuideRoute } from './src/guideVersions.mjs';

// Hand-written links and static redirect targets must include the GitHub Pages base.
const site = 'https://kimsoungryoul.github.io';
const base = '/ag-ui-rust';

// Keep labels and page order in one place across the published guide trees.
const docSections = [
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
			{ label: 'Crates and features', translations: { ko: 'crate와 feature 선택' }, link: '/start/crates/' },
		],
	},
	{
		label: 'Connect an agent (server)',
		translations: { ko: 'Agent에 AG-UI 연결하기' },
		items: [
			{ label: 'Integration quickstart', translations: { ko: '연결 시작' }, link: '/server/' },
			{ label: 'Agent and run context', translations: { ko: 'Agent와 실행 컨텍스트' }, link: '/server/agent/' },
			{ label: 'Runs that outlive a response', translations: { ko: '응답 이후에도 계속되는 run' }, link: '/server/detached-runs/' },
			{ label: 'HTTP endpoint', translations: { ko: 'HTTP endpoint' }, link: '/server/axum/' },
			{ label: 'Streaming text', translations: { ko: '텍스트 스트리밍' }, link: '/server/text/' },
			{ label: 'Tool calls and results', translations: { ko: '도구 호출·결과 전달' }, link: '/server/tools/' },
			{ label: 'Shared state', translations: { ko: '공유 상태' }, link: '/server/state/' },
			{
				label: 'Human in the loop',
				translations: { ko: '승인 요청과 재개' },
				link: '/server/interrupts/',
			},
			{ label: 'Subagent output and status', translations: { ko: '하위 agent 출력·상태 전달' }, link: '/server/subagents/' },
			{
				label: 'Errors and cancellation',
				translations: { ko: '오류와 실행 중지' },
				link: '/server/errors/',
			},
		],
	},
	{
		label: 'Call an agent (Rust client)',
		translations: { ko: 'Rust에서 Agent 호출하기' },
		items: [
			{ label: 'Client quickstart', translations: { ko: '호출 시작' }, link: '/client/' },
			{ label: 'Connect and manage threads', translations: { ko: '연결과 대화 관리' }, link: '/client/thread/' },
			{
				label: 'The update stream',
				translations: { ko: '업데이트 처리' },
				link: '/client/updates/',
			},
			{
				label: 'Rendering a run',
				translations: { ko: '메시지와 하위 agent 렌더링' },
				link: '/client/rendering/',
			},
			{ label: 'Client tools and results', translations: { ko: 'Client 도구와 결과 전달' }, link: '/client/tools/' },
			{ label: 'Shared state', translations: { ko: '공유 상태 읽기' }, link: '/client/state/' },
			{ label: 'Approvals and recovery', translations: { ko: '승인 응답과 복원' }, link: '/client/interrupts/' },
			{ label: 'Transports', translations: { ko: 'Transport 선택' }, link: '/client/transports/' },
		],
	},
	{
		label: 'A2UI',
		collapsed: true,
		translations: { ko: 'A2UI' },
		items: [
			{ label: 'Overview', translations: { ko: '개요' }, link: '/a2ui/' },
			{ label: 'Authoring surfaces', translations: { ko: 'surface 작성' }, link: '/a2ui/authoring/' },
			{ label: 'Agent layer', translations: { ko: 'agent 계층' }, link: '/a2ui/agent/' },
			{ label: 'Protocol versions', translations: { ko: '프로토콜 버전' }, link: '/a2ui/versions/' },
			{ label: 'Function calls', translations: { ko: '함수 호출' }, link: '/a2ui/functions/' },
			{ label: 'Validation', translations: { ko: 'validation' }, link: '/a2ui/validation/' },
		],
	},
	{
		label: 'Design and testing',
		collapsed: true,
		translations: { ko: '설계와 테스트' },
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
		translations: { ko: '레퍼런스' },
		items: [
			{
				label: 'Event reference',
				translations: { ko: 'Event 목록' },
				link: '/reference/events/',
			},
			{ label: 'Feature flags', translations: { ko: 'Feature 선택' }, link: '/reference/features/' },
			{
				label: 'Platforms and MSRV',
				translations: { ko: '플랫폼과 Rust 버전' },
				link: '/reference/platforms/',
			},
			{
				label: 'API docs (rustdoc)',
				translations: { ko: 'API 문서 (rustdoc)' },
				link: 'https://docs.rs/ag-ui/',
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
			{ label: 'review-desk', link: '/examples/review-desk/' },
		],
	},
];

const guidePaths = docSections.flatMap((section) =>
	section.items.map((item) => item.link).filter((link) => link.startsWith('/') && hasGuidePage(publishedVersion, link.slice(1)))
);
const legacyRedirects = Object.fromEntries(
	['', 'ko/'].flatMap((locale) =>
		guidePaths.map((path) => [
			`/${locale}${path.slice(1)}`,
			`${base}/${locale}${publishedVersion}${path}`,
		])
	)
);

function versionedSections(version: string) {
	return docSections.map((section) => ({
		...section,
		items: section.items.filter(({ link }) => hasGuidePage(version, link.slice(1))).map((item) => ({
			...item,
			link: item.label === 'API docs (rustdoc)'
				? `https://docs.rs/ag-ui/${version.slice(1)}/ag_ui/`
				: item.link.startsWith('/')
					? `/${version}${item.link}`
					: item.link,
		})),
	}));
}

export default defineConfig({
	site,
	base,
	redirects: {
		'/': `${base}/${publishedVersion}/start/`,
		'/ko/': `${base}/ko/${publishedVersion}/start/`,
		...legacyRedirects,
	},
	integrations: [
		starlight({
			title: 'ag-ui-rust',
			description:
				'A Rust SDK for the AG-UI protocol — build agent backends and agent clients in Rust.',
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ko: { label: '한국어', lang: 'ko' },
			},
			components: {
				SiteTitle: './src/components/SiteTitle.astro',
				Banner: './src/components/VersionBanner.astro',
				LanguageSelect: './src/components/LanguageSelect.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
			},
			routeMiddleware: './src/routeData.ts',
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
			lastUpdated: true,
			customCss: ['./src/styles/custom.css'],
			expressiveCode: {
				plugins: [{
					name: 'rustdoc-snippets',
					hooks: {
						preprocessLanguage: ({ codeBlock }) => {
							if (codeBlock.language.startsWith('rust,')) codeBlock.language = 'rust';
						},
						preprocessCode: ({ codeBlock }) => {
							if (codeBlock.language !== 'rust') return;
							// rustdoc setup lines stay in the compiled source, outside the displayed example.
							const lines = codeBlock.getLines();
							for (let index = lines.length - 1; index >= 0; index--) {
								const line = lines[index]!;
								if (/^\s*#(?: |$)/.test(line.text)) codeBlock.deleteLine(index);
								else if (/^\s*##/.test(line.text)) line.editText(0, undefined, line.text.replace(/^(\s*)##/, '$1#'));
							}
						},
					},
				}],
			},
			plugins: [
				starlightLinksValidator({
					failOnError: true,
					errorOnFallbackPages: false,
					// The plugin derives IDs with a slugger that removes dots from the
					// version directories. The built-page verifier checks these links.
					exclude: ({ file, link }) =>
						link.startsWith(`${base}/api/`) || Boolean(readGuideRoute(file) || readGuideRoute(link)),
				}),
			],
			sidebar: guideVersions.map((version) => ({
				label: guideSidebarLabel(version),
				collapsed: true,
				items: versionedSections(version.id),
			})),
		}),
	],
});
