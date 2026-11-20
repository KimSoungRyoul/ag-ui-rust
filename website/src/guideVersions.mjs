// The default guide stays tied to the published dependency used by quickstarts.
export const publishedVersion = 'v1.0.0';

export const guideVersions = [
  { id: publishedVersion, label: '1.0.0', status: { en: 'Stable', ko: '안정 버전' } },
  { id: 'v0.5.0-alpha.3', label: '0.5.0-alpha.3', status: { en: 'Previous prerelease', ko: '이전 프리릴리스' } },
  { id: 'v0.5.0-alpha.2', label: '0.5.0-alpha.2', status: { en: 'Previous prerelease', ko: '이전 프리릴리스' } },
  { id: 'v0.4.5', label: '0.4.5', status: { en: 'Previous stable', ko: '이전 안정 버전' } },
  { id: 'v0.5.0-alpha.1', label: '0.5.0-alpha.1', status: { en: 'Superseded', ko: '교체됨' } },
];

const pagesAddedInOne = [
  'server/detached-runs/',
  'a2ui/agent/',
  'a2ui/versions/',
  'a2ui/functions/',
];

/** @param {string} pathname */
export function readGuideRoute(pathname) {
  const segments = pathname.split('/').filter(Boolean);
  const index = segments.findIndex((segment) => guideVersions.some(({ id }) => id === segment));
  const version = guideVersions.find(({ id }) => id === segments[index]);
  if (!version) return undefined;
  return { version, page: segments.slice(index + 1).join('/') + '/' };
}

/** @param {string} version @param {string} page */
export function hasGuidePage(version, page) {
  return version === publishedVersion || !pagesAddedInOne.includes(page);
}

/** @param {string} base @param {string} locale @param {string} version @param {string} page */
export function guideHref(base, locale, version, page) {
  return `${base}/${locale}${version}/${hasGuidePage(version, page) ? page : 'start/'}`;
}

/** @param {(typeof guideVersions)[number]} version */
export function guideSidebarLabel(version) {
  return `${version.label} (${version.status.en})`;
}
