# Official A2UI schemas

Unmodified files from `a2ui-project/a2ui` at commit
`3ed9601862ecc78027e20ac80ad8b1b98a5d9079` (2026-11-14), the commit the
conformance suite under `tests/conformance/` is vendored at. Apache-2.0; the
licence is `LICENSE`.

Do not hand-edit them. To update, re-copy every file below from one newer
commit and change the commit here.

| Here | Upstream path |
|---|---|
| `v0_9/*.json` except `catalog.json` | `specification/v0_9/json/` |
| `v0_9/catalog.json` | `specification/v0_9/catalogs/basic/catalog.json` |
| `v0_9_1/*.json` except `catalog.json` | `specification/v0_9_1/json/` |
| `v0_9_1/catalog.json` | `specification/v0_9_1/catalogs/basic/catalog.json` |
| `v1_0/*.json` except `catalog.json` | `specification/v1_0/json/` |
| `v1_0/catalog.json` | `catalogs/basic/v1/catalog.json` |

`catalog.json` is the basic catalog of that version. The v0.9 and v0.9.1 basic
catalogs are two documents: v0.9 has `List.listStyle`, v0.9.1 does not.

The documents keep the ids they declare. Both v0.9-family sets declare
`https://a2ui.org/specification/v0_9/…` ids, so the two are never registered
in one schema registry. A message schema names its catalog by the relative
reference `catalog.json`; the validation registry maps that to the catalog it
was given, and never fetches a document.
