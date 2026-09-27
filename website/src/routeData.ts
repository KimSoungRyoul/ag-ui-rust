import { defineRouteMiddleware } from '@astrojs/starlight/route-data';

const latest = 'v0.5.0-alpha.2';
const versionPattern = /\/(v0\.4\.5|v0\.5\.0-alpha\.[12])\//;

export const onRequest = defineRouteMiddleware(({ url, locals }) => {
	const route = locals.starlightRoute;
	const version = url.pathname.match(versionPattern)?.[1] ?? latest;
	const activeGroup = route.sidebar.find(
		(item) => item.type === 'group' && item.label.startsWith(`${version.slice(1)} (`)
	);
	if (!activeGroup || activeGroup.type !== 'group') {
		throw new Error(`Missing documentation sidebar for ${version}`);
	}

	if (/\/(?:ko\/)?versions\/$/.test(url.pathname)) {
		route.sidebar = [];
		route.hasSidebar = false;
		route.pagination = { prev: undefined, next: undefined };
		return;
	}

	route.sidebar = activeGroup.entries;
	const pages = route.sidebar.flatMap(function flatten(item): typeof route.pagination.prev[] {
		return item.type === 'group' ? item.entries.flatMap(flatten) : [item];
	});
	const position = pages.findIndex((page) => page?.isCurrent);
	route.pagination = {
		prev: position > 0 ? pages[position - 1] : undefined,
		next: position >= 0 ? pages[position + 1] : undefined,
	};
});
