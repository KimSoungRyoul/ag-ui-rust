import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { defineCollection } from 'astro:content';

// Starlight ships its own loader and schema; declaring the collection here is
// what lets Astro type-check page frontmatter, so a stub with a missing `title`
// fails `astro check` rather than rendering blank.
export const collections = {
	docs: defineCollection({
		loader: docsLoader({
			// Astro's default slugger removes dots from version directory names.
			generateId: ({ entry, data }) =>
				typeof data.slug === 'string'
					? data.slug
					: entry.replace(/\.(?:markdown|mdown|mkdn|mkd|mdwn|md|mdx)$/, '').replace(/\/index$/, ''),
		}),
		schema: docsSchema(),
	}),
};
