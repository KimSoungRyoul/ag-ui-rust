import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { defineCollection } from 'astro:content';

// Starlight ships its own loader and schema; declaring the collection here is
// what lets Astro type-check page frontmatter, so a stub with a missing `title`
// fails `astro check` rather than rendering blank.
export const collections = {
	docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
};
