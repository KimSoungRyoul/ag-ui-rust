import { defineRouteMiddleware } from '@astrojs/starlight/route-data';

import { guideVersions, guideSidebarLabel, publishedVersion, readGuideRoute } from './guideVersions.mjs';

const defaultGuide = guideVersions.find(({ id }) => id === publishedVersion)!;

export const onRequest = defineRouteMiddleware(({ url, locals }) => {
	const route = locals.starlightRoute;
	const guide = readGuideRoute(url.pathname);
	const version = guide?.version ?? defaultGuide;
	const activeGroup = route.sidebar.find(
		(item) => item.type === 'group' && item.label === guideSidebarLabel(version)
	);
	if (!activeGroup || activeGroup.type !== 'group') {
		throw new Error(`Missing documentation sidebar for ${version.id}`);
	}

	if (!guide && url.pathname.endsWith('/versions/')) {
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
