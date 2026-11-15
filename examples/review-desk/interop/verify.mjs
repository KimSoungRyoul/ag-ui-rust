import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { MessageProcessor as ProcessorV09 } from "@a2ui/web_core/v0_9";
import { basicCatalog as basicCatalogV09 } from "@a2ui/web_core/v0_9/basic_catalog";
import { MessageProcessor as ProcessorV10 } from "@a2ui/web_core/v1_0";
import { basicCatalog as basicCatalogV10 } from "@a2ui/web_core/catalogs/basic/v1";

const RENDERER = "@a2ui/web_core@0.15.0";
const root = fileURLToPath(new URL("../../../", import.meta.url));

// What the Rust SDK exports for one protocol version. With no version the
// command is the one it has always been, and exports v0.9.1.
function exported(version) {
  const args = ["run", "--locked", "--quiet", "-p", "review-desk", "--", "export-a2ui"];
  if (version) args.push(version);
  return JSON.parse(execFileSync("cargo", args, {
    cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"],
  }));
}

// A lossless representation for comparing ordinary objects, explicit null and
// JavaScript undefined/sparse slots to the SDK's local snapshot, never wire data.
function modelValue(value) {
  if (value === undefined) return {kind: "undefined"};
  if (value === null) return {kind: "null"};
  if (Array.isArray(value)) return {kind: "array", value: Array.from(value, modelValue)};
  if (typeof value === "object") return {kind: "object", value: Object.fromEntries(
    Object.entries(value).map(([key, value]) => [key, modelValue(value)]),
  )};
  return {kind: typeof value === "boolean" ? "bool" : typeof value, value};
}

// What a surface is to a renderer: its components, by id.
function components(surface) {
  if (!surface) return null;
  return [...surface.componentsModel.entries]
    .map(([id, component]) => [id, component.type, component.properties])
    .sort(([a], [b]) => a.localeCompare(b));
}

function snapshot(surface) {
  return surface
    ? {snapshot_version: 1, root: modelValue(surface.dataModel.get("/"))}
    : null;
}

// Where this renderer and the v0.9 specification part.
//
// In v0.9 a message removes by leaving `value` out, and `null` is a value
// (specification/v0_9/docs/a2ui_protocol.md, "Server to client updates"). The
// SDK stores it. @a2ui/web_core 0.15.0 applies an `updateDataModel` with
// `DataModel.set(path, value)` whatever the surface's version
// (src/processing/message-processor.js, processUpdateDataModelOp), and `set`
// treats `null` as a removal unless it is told to keep it
// (src/state/data-model.js: "Pre-1.0 data models stored `null`; the v1.0
// protocol removes the entry"), which the processor does not tell it. So a
// `null` written at a path of a v0.9 surface is gone from this renderer and
// held by the SDK. A `null` inside a written value is stored by both. In
// v1.0 `null` removes for both, and there is nothing to reconcile.
//
// The paths a tracker holds are the ones a message wrote `null` at and
// nothing has written since. The SDK's snapshot is compared as this renderer
// holds it: without those keys, and with those list slots undefined.
function nullTracker(storesNull) {
  const written = new Set();
  return {
    written,
    note(messages) {
      if (!storesNull) return;
      for (const message of messages) {
        if (message.createSurface || message.deleteSurface) written.clear();
        const update = message.updateDataModel;
        if (!update) continue;
        const path = update.path ?? "/";
        if (path === "/") {
          written.clear();
          continue;
        }
        for (const held of [...written]) {
          // Overwritten, removed with what holds it, or written through.
          if (held === path || held.startsWith(`${path}/`) || path.startsWith(`${held}/`)) {
            written.delete(held);
          }
        }
        if (Object.hasOwn(update, "value") && update.value === null) written.add(path);
      }
    },
    asThisRendererHolds(sdkData) {
      if (!sdkData || written.size === 0) return sdkData;
      const held = structuredClone(sdkData);
      for (const path of written) {
        const segments = path.split("/").slice(1)
          .map(segment => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
        const last = segments.pop();
        let node = held.root;
        for (const segment of segments) node = node?.value?.[segment];
        if (node?.value?.[last]?.kind !== "null") continue;
        if (node.kind === "array") node.value[last] = {kind: "undefined"};
        else delete node.value[last];
      }
      return held;
    },
  };
}

function verify(fixture, MessageProcessor, basicCatalog) {
  // v1.0 writes a binding `@path` and a removal `"value": null`; the v0.9
  // family writes `path` and leaves `value` out.
  const v1 = fixture.version === "1.0";
  const binding = path => (v1 ? {"@path": path} : {path});
  const isRemoval = update => (v1
    ? Object.hasOwn(update, "value") && update.value === null
    : !Object.hasOwn(update, "value"));
  assert.equal(fixture.catalogId, basicCatalog.id);
  const processor = new MessageProcessor([basicCatalog]);
  const nulls = nullTracker(!v1);
  let original;

  for (const { name, messages, activity, sdkData } of fixture.steps) {
    assert(messages.every(message => message.version === fixture.version));

    // What an AG-UI frontend paints from is the surface activity, and it paints
    // each one on its own. So the activity alone, on a processor that holds
    // nothing, has to leave what every batch so far left.
    assert.equal(activity.activityType, "a2ui-surface");
    assert.equal(activity.replace, true);
    assert.equal(activity.messageId, fixture.steps[0].activity.messageId,
      `${name}: one run replaces one activity in place`);
    const alone = new MessageProcessor([basicCatalog]);
    alone.processMessages(activity.content.a2ui_operations);
    const painted = alone.model.getSurface(fixture.surfaceId);
    assert.deepEqual(snapshot(painted), sdkData,
      `${name}: the activity alone must paint the data the SDK holds`);

    // Pass the Rust SDK's emitted messages directly to the official processor.
    processor.processMessages(messages);
    nulls.note(messages);
    const surface = processor.model.getSurface(fixture.surfaceId);
    const data = surface?.dataModel;
    assert.deepEqual(
      snapshot(surface),
      nulls.asThisRendererHolds(sdkData),
      `${name}: SDK and official core must agree before later steps overwrite state`,
    );
    assert.deepEqual(components(painted), components(surface),
      `${name}: the activity alone must paint the components the batches left`);

    switch (name) {
      case "create":
        original = surface;
        assert.equal(surface.catalog.id, fixture.catalogId);
        assert.equal(surface.componentsModel.get("root").type, "Column");
        assert.deepEqual(surface.componentsModel.get("title").properties.text, binding("/title"));
        assert.equal(data.get("/title"), "Original review");
        assert.equal(data.get("/memo"), null);
        break;
      case "edit":
        assert.equal(data.get("/title"), "Reviewed title");
        assert.equal(data.get("/memo"), null);
        break;
      case "explicit-null":
        // A `null` written at `/memo`. In v1.0 that removes, for the SDK and
        // the renderer alike. In v0.9 the SDK holds `memo: null`, as the
        // specification says, and this renderer removed it.
        assert.equal(Object.hasOwn(data.get("/"), "memo"), false);
        if (v1) {
          assert.equal(Object.hasOwn(sdkData.root.value, "memo"), false);
        } else {
          assert.deepEqual([...nulls.written], ["/memo"]);
          assert.deepEqual(sdkData.root.value.memo, {kind: "null"});
        }
        // A `null` inside a written value is stored by both, in every version.
        assert.deepEqual(data.get("/items"), [1, null, 3]);
        assert.equal(data.get("/discard"), "temporary");
        break;
      case "upsert-missing-array":
        assert(Array.isArray(data.get("/list")));
        assert.deepEqual(data.get("/list"), ["first"]);
        break;
      case "upsert-null-parent":
        assert.deepEqual(data.get("/nullable"), {name: "nested"});
        assert.deepEqual(data.get("/nullableArray"), ["first"]);
        // Written through, the two nulls are containers in both again.
        assert.deepEqual([...nulls.written], v1 ? [] : ["/memo"]);
        break;
      case "upsert-sparse-array":
        // The third message removes `/list/5`, past the end of the list: a
        // removal of what is not there changes nothing, in every version.
        assert.equal(data.get("/list").length, 4);
        assert.deepEqual(Array.from(data.get("/list")), ["first", undefined, undefined, "fourth"]);
        assert.equal(data.get("/sparse").length, 3);
        assert.deepEqual(Array.from(data.get("/sparse")), [undefined, undefined, "third"]);
        assert(isRemoval(messages[2].updateDataModel));
        break;
      case "upsert-nested-array":
        assert(Array.isArray(data.get("/matrix")));
        assert.equal(data.get("/matrix").length, 1);
        assert(Array.isArray(data.get("/matrix/0")));
        assert.deepEqual(Array.from(data.get("/matrix/0")), [undefined, "inner"]);
        break;
      case "remove-object-key":
        assert(isRemoval(messages[0].updateDataModel));
        assert.equal(Object.hasOwn(data.get("/"), "discard"), false);
        assert.equal(data.get("/discard"), undefined);
        break;
      case "remove-array-slot":
        assert(isRemoval(messages[0].updateDataModel));
        assert.equal(data.get("/items").length, 3);
        assert.equal(data.get("/items/0"), 1);
        assert.equal(data.get("/items/1"), undefined);
        assert.equal(data.get("/items/2"), 3);
        break;
      case "replace-component":
        assert.equal(surface.componentsModel.get("title").properties.text, "Replacement text");
        assert.equal([...surface.componentsModel.entries].length, 2);
        break;
      case "upsert-null-root":
        assert.deepEqual(data.get("/"), {name: "root restored"});
        break;
      case "delete":
        assert.equal(surface, undefined);
        break;
      case "recreate":
        assert.notEqual(surface, original);
        assert.equal(surface.componentsModel.get("root").type, "Text");
        assert.equal(surface.componentsModel.get("title"), undefined);
        assert.equal(data.get("/memo"), undefined);
        assert.deepEqual(data.get("/"), fixture.finalSdkData);
        break;
      default: throw new Error(`unchecked fixture step: ${name}`);
    }
    console.log(`PASS ${fixture.version} ${name}`);
  }
  assert.equal(fixture.steps.length, 13);
  assert.equal(nulls.written.size, 0, "the last step leaves no null the two hold differently");
  console.log(`PASS ${RENDERER} consumed unmodified Rust A2UI ${fixture.version} messages`);
}

const v09 = exported();
assert.equal(v09.version, "v0.9.1");
verify(v09, ProcessorV09, basicCatalogV09);

const v10 = exported("1.0");
assert.equal(v10.version, "1.0");
verify(v10, ProcessorV10, basicCatalogV10);

console.log("PASS each a2ui-surface activity painted the same surface on its own");
