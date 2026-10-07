"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const { PassThrough } = require("node:stream");
const shim = require("./shim.js");

const files = (list) => (p) => list.includes(p);
const versions = (map) => (p) => (p in map ? map[p] : null);

test("macOS looks at the two Homebrew paths first, then PATH", () => {
  assert.deepEqual(shim.candidates("darwin", { PATH: "/usr/bin:/Users/ada/bin" }), [
    "/opt/homebrew/bin/crystalline",
    "/usr/local/bin/crystalline",
    "/usr/bin/crystalline",
    "/Users/ada/bin/crystalline",
  ]);
});

test("Windows looks at Program Files, then PATH, and only for crystalline.exe", () => {
  const env = {
    ProgramFiles: "C:\\Program Files",
    "ProgramFiles(x86)": "C:\\Program Files (x86)",
    Path: "C:\\Windows\\System32;C:\\Tools",
  };
  assert.deepEqual(shim.candidates("win32", env), [
    "C:\\Program Files\\Crystalline\\bin\\crystalline.exe",
    "C:\\Program Files (x86)\\Crystalline\\bin\\crystalline.exe",
    "C:\\Windows\\System32\\crystalline.exe",
    "C:\\Tools\\crystalline.exe",
  ]);
});

test("the version line is read, and anything else is not Crystalline", () => {
  assert.deepEqual(shim.parseVersion("crystalline 0.24.0").triple, [0, 24, 0]);
  assert.deepEqual(shim.parseVersion("crystalline 0.25.0-dev.4817").triple, [0, 25, 0]);
  assert.equal(shim.parseVersion("crystalline: command for crystals 1.0"), null);
  assert.equal(shim.parseVersion("Crystalline 0.24.0 something"), null);
  assert.equal(shim.parseVersion(""), null);
  assert.equal(shim.parseVersion(null), null);
});

test("a dev build with a pre-release suffix passes the gate", () => {
  assert.ok(shim.atLeast(shim.parseVersion("crystalline 0.24.0-dev.17").triple, shim.MINIMUM));
  assert.ok(shim.atLeast([0, 25, 0], shim.MINIMUM));
  assert.ok(!shim.atLeast([0, 23, 1], shim.MINIMUM));
  assert.ok(!shim.atLeast([0, 9, 99], shim.MINIMUM));
});

test("the first Crystalline found decides", () => {
  const found = shim.findBinary({
    platform: "darwin",
    env: { PATH: "/usr/bin" },
    isFile: files(["/opt/homebrew/bin/crystalline", "/usr/bin/crystalline"]),
    runVersion: versions({
      "/opt/homebrew/bin/crystalline": "crystalline 0.24.0",
      "/usr/bin/crystalline": "crystalline 9.9.9",
    }),
  });
  assert.deepEqual(found, { kind: "ok", path: "/opt/homebrew/bin/crystalline", version: "0.24.0" });
});

test("a crystalline on PATH that prints something else is skipped", () => {
  const found = shim.findBinary({
    platform: "darwin",
    env: { PATH: "/usr/bin:/Users/ada/bin" },
    isFile: files(["/usr/bin/crystalline", "/Users/ada/bin/crystalline"]),
    runVersion: versions({
      "/usr/bin/crystalline": "usage: crystalline [-x] file",
      "/Users/ada/bin/crystalline": "crystalline 0.24.0",
    }),
  });
  assert.equal(found.path, "/Users/ada/bin/crystalline");
});

test("a candidate whose version cannot be read is skipped", () => {
  const found = shim.findBinary({
    platform: "darwin",
    env: { PATH: "" },
    isFile: files(["/opt/homebrew/bin/crystalline"]),
    runVersion: () => null,
  });
  assert.equal(found.kind, "missing");
});

const unix = { skip: process.platform === "win32" && "the fixtures run through a shebang" };

test("the version check reads the first line of a real run", unix, () => {
  assert.equal(shim.runVersion(path.join(__dirname, "fixtures", "quick-version.js")), "crystalline 0.24.0");
});

test("a version check that does not answer within its timeout is not Crystalline", unix, () => {
  const began = Date.now();
  const line = shim.runVersion(path.join(__dirname, "fixtures", "slow-version.js"), 200);
  assert.equal(line, null);
  assert.ok(Date.now() - began < 2500, "the check gave up before the fixture printed");
});

test("an old Crystalline is reported with its path and version", () => {
  const found = shim.findBinary({
    platform: "win32",
    env: { ProgramFiles: "C:\\Program Files" },
    isFile: files(["C:\\Program Files\\Crystalline\\bin\\crystalline.exe"]),
    runVersion: () => "crystalline 0.23.1",
  });
  assert.deepEqual(found, {
    kind: "old",
    path: "C:\\Program Files\\Crystalline\\bin\\crystalline.exe",
    version: "0.23.1",
  });
});

test("with no binary on macOS the stub names brew and the Desktop-only bundle", () => {
  const note = shim.notice({ kind: "missing" }, "darwin", "0.24.0");
  assert.match(note.instructions, /brew install jordiboehme\/tap\/crystalline/);
  assert.match(note.instructions, /crystalline-desktop-only-v0\.24\.0-macos\.mcpb/);
  assert.match(note.instructions, /restart Claude Desktop/);
  assert.equal(note.payload.available, false);
  assert.equal(note.payload.needed_version, "0.24.0");
});

test("with no binary on Windows the stub names the MSI", () => {
  const note = shim.notice({ kind: "missing" }, "win32", "0.24.0");
  assert.match(note.instructions, /MSI/);
  assert.match(note.instructions, /https:\/\/github\.com\/jordiboehme\/crystalline\/releases/);
  assert.doesNotMatch(note.instructions, /brew/);
});

test("too old names both versions and the update", () => {
  const mac = shim.notice({ kind: "old", path: "/opt/homebrew/bin/crystalline", version: "0.23.1" }, "darwin", "0.24.0");
  assert.match(mac.instructions, /v0\.23\.1/);
  assert.match(mac.instructions, /v0\.24\.0 or newer/);
  assert.match(mac.fix, /brew upgrade crystalline/);
  assert.equal(mac.payload.found_version, "0.23.1");
  const win = shim.notice({ kind: "old", path: "C:\\x\\crystalline.exe", version: "0.22.0" }, "win32", "0.24.0");
  assert.match(win.fix, /newest MSI/);
});

test("no sentence carries an em or en dash", () => {
  for (const outcome of [{ kind: "missing" }, { kind: "old", path: "/p", version: "0.1.0" }]) {
    for (const platform of ["darwin", "win32"]) {
      const note = shim.notice(outcome, platform, "0.24.0");
      const text = note.instructions + note.fix + JSON.stringify(note.payload);
      assert.doesNotMatch(text, /[\u2013\u2014]/);
    }
  }
});

const note = shim.notice({ kind: "missing" }, "darwin", "0.24.0");
const era = { "io.modelcontextprotocol/protocolVersion": "2026-07-28" };

test("the stub answers initialize like the Rust stub", () => {
  const reply = shim.stubAnswer(
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } },
    note,
  );
  assert.equal(reply.id, 1);
  assert.equal(reply.result.protocolVersion, "2025-06-18");
  assert.deepEqual(reply.result.capabilities, { tools: {} });
  assert.equal(reply.result.serverInfo.name, "crystalline");
  assert.equal(reply.result.serverInfo.version, "0.24.0");
  assert.equal(reply.result.instructions, note.instructions);
  for (const asked of ["2026-07-28", "1999-01-01", undefined]) {
    const r = shim.stubAnswer({ jsonrpc: "2.0", id: 2, method: "initialize", params: { protocolVersion: asked } }, note);
    assert.equal(r.result.protocolVersion, "2025-11-25", String(asked));
  }
});

test("the stub answers server/discover with the era's shape", () => {
  const reply = shim.stubAnswer({ jsonrpc: "2.0", id: "d", method: "server/discover", params: {} }, note);
  assert.equal(reply.result.resultType, "complete");
  assert.deepEqual(reply.result.supportedVersions, [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
  ]);
  assert.equal(reply.result.instructions, note.instructions);
  assert.equal(reply.result.ttlMs, 0);
  assert.equal(reply.result.cacheScope, "private");
  assert.equal(reply.result._meta["io.modelcontextprotocol/serverInfo"].name, "crystalline");
});

test("tools/list is the one status tool, with hints only for the era", () => {
  const legacy = shim.stubAnswer({ jsonrpc: "2.0", id: 3, method: "tools/list" }, note);
  assert.equal(legacy.result.tools.length, 1);
  const tool = legacy.result.tools[0];
  assert.equal(tool.name, "status");
  assert.deepEqual(tool.inputSchema, { type: "object", properties: {} });
  assert.equal(tool.annotations.readOnlyHint, true);
  assert.equal(tool.annotations.openWorldHint, false);
  assert.equal(legacy.result.ttlMs, undefined);
  const modern = shim.stubAnswer({ jsonrpc: "2.0", id: 4, method: "tools/list", params: { _meta: era } }, note);
  assert.equal(modern.result.resultType, "complete");
  assert.equal(modern.result.ttlMs, 0);
  // The Rust stub's lists carry the server's public scope (mcp.rs
  // CACHE_SCOPE); only server/discover is private.
  assert.equal(modern.result.cacheScope, "public");
});

test("tools/call of status returns the payload, any other tool an error with the fix", () => {
  const ok = shim.stubAnswer({ jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "status", arguments: {} } }, note);
  assert.equal(ok.result.isError, false);
  assert.deepEqual(JSON.parse(ok.result.content[0].text), note.payload);
  const stale = shim.stubAnswer({ jsonrpc: "2.0", id: 6, method: "tools/call", params: { name: "search_engrams" } }, note);
  assert.equal(stale.result.isError, true);
  assert.match(stale.result.content[0].text, /brew install/);
});

test("ping is answered only on the legacy lifecycle", () => {
  assert.deepEqual(shim.stubAnswer({ jsonrpc: "2.0", id: 7, method: "ping" }, note).result, {});
  const modern = shim.stubAnswer({ jsonrpc: "2.0", id: 8, method: "ping", params: { _meta: era } }, note);
  assert.equal(modern.error.code, -32601);
});

test("empty lists, unknown methods and notifications", () => {
  assert.deepEqual(shim.stubAnswer({ jsonrpc: "2.0", id: 9, method: "resources/list" }, note).result, { resources: [] });
  assert.deepEqual(shim.stubAnswer({ jsonrpc: "2.0", id: 10, method: "prompts/list" }, note).result, { prompts: [] });
  assert.deepEqual(
    shim.stubAnswer({ jsonrpc: "2.0", id: 11, method: "resources/templates/list" }, note).result,
    { resourceTemplates: [] },
  );
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", id: 12, method: "sampling/whatever" }, note).error.code, -32601);
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", method: "notifications/initialized" }, note), null);
  assert.equal(shim.stubAnswer("not an object", note), null);
});

test("the stub serves line by line and answers a parse error", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const lines = [];
  output.on("data", (chunk) => lines.push(...chunk.toString().split("\n").filter(Boolean)));
  const done = new Promise((resolve) => output.on("finish", resolve));
  shim.serveStub(note, input, output);
  input.write('{"jsonrpc":"2.0","id":1,"method":"ping"}\n');
  input.write("{not json\n");
  input.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n');
  input.end();
  await done;
  assert.equal(lines.length, 2);
  assert.deepEqual(JSON.parse(lines[0]).result, {});
  assert.equal(JSON.parse(lines[1]).error.code, -32700);
});

test("the relay passes bytes through unchanged and exits with the child's code", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const errors = new PassThrough();
  const out = [];
  const err = [];
  output.on("data", (c) => out.push(c));
  errors.on("data", (c) => err.push(c));
  const exited = new Promise((resolve) => {
    shim.start(process.execPath, [path.join(__dirname, "fixtures", "echo-child.js")], process.env, {
      input,
      output,
      errors,
      exit: resolve,
    });
  });
  const bytes = Buffer.from([0x7b, 0x22, 0xc3, 0xa9, 0x22, 0x7d, 0x0d, 0x0a, 0xff, 0x00, 0x0a]);
  input.write(bytes);
  input.end();
  const code = await exited;
  assert.equal(code, 3);
  assert.deepEqual(Buffer.concat(out), bytes);
  assert.equal(Buffer.concat(err).toString(), "child stderr line\n");
});

test("a binary that cannot start is reported once and exits with 1", async () => {
  const errors = new PassThrough();
  const err = [];
  errors.on("data", (c) => err.push(c));
  const codes = [];
  await new Promise((resolve) => {
    shim.start(path.join(__dirname, "fixtures", "no-such-binary"), ["mcp"], process.env, {
      input: new PassThrough(),
      output: new PassThrough(),
      errors,
      exit: (code) => {
        codes.push(code);
        setTimeout(resolve, 100);
      },
    });
  });
  assert.deepEqual(codes, [1]);
  assert.match(Buffer.concat(err).toString(), /could not start/);
});

test("Windows strips quotes from PATH entries and looks at ProgramW6432 first", () => {
  const env = {
    ProgramW6432: "C:\\Program Files",
    ProgramFiles: "C:\\Program Files (x86)",
    "ProgramFiles(x86)": "C:\\Program Files (x86)",
    Path: '"C:\\My Tools;2";C:\\Tools;""',
  };
  assert.deepEqual(shim.candidates("win32", env), [
    "C:\\Program Files\\Crystalline\\bin\\crystalline.exe",
    "C:\\Program Files (x86)\\Crystalline\\bin\\crystalline.exe",
    "C:\\My Tools;2\\crystalline.exe",
    "C:\\Tools\\crystalline.exe",
  ]);
  assert.deepEqual(shim.candidates("win32", { Path: '"C:\\Quoted Dir";C:\\Tools' }), [
    "C:\\Quoted Dir\\crystalline.exe",
    "C:\\Tools\\crystalline.exe",
  ]);
});

test("a version check that exits non-zero is not Crystalline", unix, () => {
  assert.equal(shim.runVersion(path.join(__dirname, "fixtures", "failing-version.js")), null);
});

test("a version check whose first line is something else is not Crystalline", unix, () => {
  const line = shim.runVersion(path.join(__dirname, "fixtures", "garbage-version.js"));
  assert.equal(line, "usage: crystalline [-x] file");
  assert.equal(shim.parseVersion(line), null);
});

test("a JSON-RPC response sent to the stub is not answered", () => {
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", id: 1, result: {} }, note), null);
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", id: 2, error: { code: -1, message: "x" } }, note), null);
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", id: 3, method: 7 }, note), null);
});

test("a revision after the era keeps the era's rules", () => {
  const later = { "io.modelcontextprotocol/protocolVersion": "2027-01-01" };
  const listed = shim.stubAnswer({ jsonrpc: "2.0", id: 1, method: "tools/list", params: { _meta: later } }, note);
  assert.equal(listed.result.resultType, "complete");
  assert.equal(listed.result.ttlMs, 0);
  assert.equal(shim.stubAnswer({ jsonrpc: "2.0", id: 2, method: "ping", params: { _meta: later } }, note).error.code, -32601);
  const older = { "io.modelcontextprotocol/protocolVersion": "2025-11-25" };
  assert.deepEqual(shim.stubAnswer({ jsonrpc: "2.0", id: 3, method: "ping", params: { _meta: older } }, note).result, {});
});

const { EventEmitter } = require("node:events");
const gone = (pid) => {
  try {
    process.kill(pid, 0);
    return false;
  } catch (_) {
    return true;
  }
};
const relayed = (fixture) => {
  const io = { input: new PassThrough(), output: new PassThrough(), errors: new PassThrough() };
  io.output.resume();
  io.errors.resume();
  let child;
  const exited = new Promise((resolve) => {
    child = shim.start(process.execPath, [path.join(__dirname, "fixtures", fixture)], process.env, {
      ...io,
      exit: resolve,
    });
  });
  return { child, exited, io };
};

test("a forwarded stop signal ends the relayed child", unix, async () => {
  const { child, exited } = relayed("echo-child.js");
  const signals = new EventEmitter();
  shim.forwardSignals(child, signals, "darwin", 200);
  await new Promise((resolve) => child.stderr.once("data", resolve));
  signals.emit("SIGTERM");
  assert.equal(await exited, 1);
  assert.ok(gone(child.pid), "the child is gone");
});

test("a child that ignores the stop signal is killed after the grace period", unix, async () => {
  const { child, exited } = relayed("stubborn-child.js");
  await new Promise((resolve) => child.stdout.once("data", resolve));
  const began = Date.now();
  shim.stopChild(child, "SIGTERM", 200);
  assert.equal(await exited, 1);
  assert.equal(child.signalCode, "SIGKILL");
  assert.ok(Date.now() - began < 2000);
  assert.ok(gone(child.pid), "the child is gone");
});

test("no signal is forwarded on Windows", () => {
  const signals = new EventEmitter();
  shim.forwardSignals({ kill: () => assert.fail("no kill") }, signals, "win32");
  assert.equal(signals.listenerCount("SIGTERM"), 0);
});

test("a closed output pipe stops the child quietly", unix, async () => {
  const { child, exited, io } = relayed("stubborn-child.js");
  await new Promise((resolve) => child.stdout.once("data", resolve));
  const pipeError = Object.assign(new Error("write EPIPE"), { code: "EPIPE" });
  io.output.destroy(pipeError);
  assert.equal(await exited, 1);
  assert.ok(gone(child.pid), "the child is gone");
});
