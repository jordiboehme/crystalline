"use strict";
// The Claude Desktop extension's launcher. Claude Desktop runs this file with
// its own built-in Node. It finds the Crystalline that Homebrew (macOS) or
// the MSI (Windows) installed, checks that it is new enough, and relays
// stdio to `crystalline mcp` byte for byte. Without a usable binary it
// answers MCP itself, with one `status` tool that says what to install.
//
// Plain CommonJS with no dependencies, so it runs on any Node Claude Desktop
// ships. It writes no files and keeps no state.

const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");
const readline = require("readline");

const MINIMUM = [0, 24, 0];
const MINIMUM_TEXT = MINIMUM.join(".");
const RELEASES = "https://github.com/jordiboehme/crystalline/releases";
const SERVED_VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"];
const ERA = "2026-07-28";
const NEWEST_WITH_INITIALIZE = "2025-11-25";
const VERSION_KEY = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_KEY = "io.modelcontextprotocol/serverInfo";
// The Rust stub's list results carry the healthy server's public scope;
// only server/discover is private, as rmcp builds it.
const LIST_SCOPE = "public";
const DISCOVER_SCOPE = "private";
const STATUS_TOOL = {
  name: "status",
  title: "Crystalline status",
  description:
    "Report why Crystalline is degraded this session: the startup failure, this binary's version, the daemon that owns the knowledge index and how to fix it. Relay the fix to the user.",
  inputSchema: { type: "object", properties: {} },
  annotations: { title: "Crystalline status", readOnlyHint: true, openWorldHint: false },
};

/**
 * The folders of a Windows PATH. An entry may stand in double quotes, which
 * Windows drops, and a `;` inside the quotes is part of the folder name.
 * When the quotes do not balance, every `;` splits, and an entry left with a
 * stray quote is not absolute, so `candidates` skips it.
 */
function windowsPath(value) {
  if ((value.match(/"/g) || []).length % 2 !== 0) return value.split(";").filter(Boolean);
  const dirs = [];
  let dir = "";
  let quoted = false;
  for (const char of value) {
    if (char === '"') quoted = !quoted;
    else if (char === ";" && !quoted) {
      dirs.push(dir);
      dir = "";
    } else dir += char;
  }
  dirs.push(dir);
  return dirs.filter(Boolean);
}

/**
 * Where to look, in order: the first hit that is Crystalline wins. A PATH
 * entry that is not absolute is skipped, so nothing is resolved against
 * Claude Desktop's working folder.
 */
function candidates(platform, env) {
  const list = [];
  if (platform === "win32") {
    // ProgramW6432 names the 64-bit Program Files even when this Node is a
    // 32-bit process, where ProgramFiles points at the (x86) folder.
    for (const root of [env.ProgramW6432, env.ProgramFiles, env["ProgramFiles(x86)"]]) {
      if (root) list.push(path.win32.join(root, "Crystalline", "bin", "crystalline.exe"));
    }
    for (const dir of windowsPath(String(env.PATH || env.Path || ""))) {
      if (path.win32.isAbsolute(dir)) list.push(path.win32.join(dir, "crystalline.exe"));
    }
  } else {
    list.push("/opt/homebrew/bin/crystalline", "/usr/local/bin/crystalline");
    for (const dir of String(env.PATH || "").split(":")) {
      if (path.posix.isAbsolute(dir)) list.push(path.posix.join(dir, "crystalline"));
    }
  }
  return [...new Set(list)];
}

/** `crystalline 0.24.0` or `crystalline 0.25.0-dev.4817`; anything else is not Crystalline. */
function parseVersion(firstLine) {
  const match = /^crystalline (\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?$/.exec(String(firstLine || "").trim());
  if (!match) return null;
  const triple = [Number(match[1]), Number(match[2]), Number(match[3])];
  return { triple, text: `${triple.join(".")}${match[4] || ""}` };
}

/** The numeric triple only: a pre-release of a new enough version passes. */
function atLeast(triple, minimum) {
  for (let i = 0; i < 3; i += 1) {
    if (triple[i] !== minimum[i]) return triple[i] > minimum[i];
  }
  return true;
}

function findBinary({ platform, env, isFile, runVersion }) {
  for (const candidate of candidates(platform, env)) {
    if (!isFile(candidate)) continue;
    const parsed = parseVersion(runVersion(candidate));
    if (!parsed) continue;
    return { kind: atLeast(parsed.triple, MINIMUM) ? "ok" : "old", path: candidate, version: parsed.text };
  }
  return { kind: "missing" };
}

function isFile(candidate) {
  try {
    return fs.statSync(candidate).isFile();
  } catch (_) {
    return false;
  }
}

/**
 * The first line of `<binary> --version`, or null when it fails or takes
 * longer than `timeoutMs` (5 s unless a test asks for less).
 */
function runVersion(binary, timeoutMs = 5000) {
  try {
    const out = childProcess.execFileSync(binary, ["--version"], {
      timeout: timeoutMs,
      encoding: "utf8",
      windowsHide: true,
      stdio: ["ignore", "pipe", "ignore"],
    });
    return out.split(/\r?\n/)[0];
  } catch (_) {
    return null;
  }
}

/** What the stub says, per outcome and platform. */
function notice(outcome, platform, extensionVersion) {
  const mac = platform === "darwin";
  let reason;
  let instructions;
  let fix;
  if (outcome.kind === "old") {
    const update = mac ? "`brew upgrade crystalline`" : `the newest MSI from ${RELEASES}`;
    reason = `the Crystalline at ${outcome.path} is v${outcome.version}, and this extension needs v${MINIMUM_TEXT} or newer`;
    instructions = `Crystalline is not ready this session: ${reason}. No knowledge tools are available. Ask the user to update it with ${update}, then restart Claude Desktop. Call the status tool for the full details to relay.`;
    fix = `Update Crystalline with ${update}, then restart Claude Desktop.`;
  } else if (mac) {
    reason = "Crystalline is not installed on this Mac";
    instructions = `Crystalline is not ready this session: ${reason}, and this extension only connects Claude Desktop to it. No knowledge tools are available. Ask the user to install it with \`brew install jordiboehme/tap/crystalline\` and restart Claude Desktop. Without Homebrew, they can install crystalline-desktop-only-v${extensionVersion}-macos.mcpb from ${RELEASES} instead of this extension. Call the status tool for the full details to relay.`;
    fix = `Run \`brew install jordiboehme/tap/crystalline\` and restart Claude Desktop, or install crystalline-desktop-only-v${extensionVersion}-macos.mcpb from ${RELEASES} instead of this extension.`;
  } else {
    reason = "Crystalline is not installed on this computer";
    instructions = `Crystalline is not ready this session: ${reason}, and this extension only connects Claude Desktop to it. No knowledge tools are available. Ask the user to download the MSI from ${RELEASES}, run it, and restart Claude Desktop. Call the status tool for the full details to relay.`;
    fix = `Download and run the MSI from ${RELEASES}, then restart Claude Desktop.`;
  }
  const payload = {
    available: false,
    reason,
    extension_version: extensionVersion,
    needed_version: MINIMUM_TEXT,
    fix,
  };
  if (outcome.path) payload.found_path = outcome.path;
  if (outcome.version) payload.found_version = outcome.version;
  return { instructions, fix, payload, extensionVersion };
}

function failure(id, code, message) {
  return { jsonrpc: "2.0", id, error: { code, message } };
}

/**
 * The stub's answer to one message, or null for a notification and for a
 * response (a message with an id but no method), which is never answered.
 */
function stubAnswer(message, note) {
  if (!message || typeof message !== "object" || message.id === undefined || message.id === null) {
    return null;
  }
  if (typeof message.method !== "string") return null;
  const { id, method } = message;
  const params = message.params && typeof message.params === "object" ? message.params : {};
  const meta = params._meta && typeof params._meta === "object" ? params._meta : {};
  // Revisions are ISO dates, so a string compare orders them. A revision
  // after the era keeps the era's rules, as in the Rust server.
  const asked = meta[VERSION_KEY];
  const modern = typeof asked === "string" && asked >= ERA;
  const ok = (result) => ({ jsonrpc: "2.0", id, result });
  const listed = (result) =>
    ok(modern ? Object.assign({ resultType: "complete", ttlMs: 0, cacheScope: LIST_SCOPE }, result) : result);
  const serverInfo = { name: "crystalline", version: note.extensionVersion };
  switch (method) {
    case "initialize": {
      const wanted = params.protocolVersion;
      const answered = SERVED_VERSIONS.includes(wanted) && wanted < ERA ? wanted : NEWEST_WITH_INITIALIZE;
      return ok({ protocolVersion: answered, capabilities: { tools: {} }, serverInfo, instructions: note.instructions });
    }
    case "server/discover":
      return ok({
        resultType: "complete",
        supportedVersions: SERVED_VERSIONS,
        capabilities: { tools: {} },
        instructions: note.instructions,
        ttlMs: 0,
        cacheScope: DISCOVER_SCOPE,
        _meta: { [SERVER_INFO_KEY]: serverInfo },
      });
    case "ping":
      return modern ? failure(id, -32601, "ping is not part of this protocol revision") : ok({});
    case "tools/list":
      return listed({ tools: [STATUS_TOOL] });
    case "tools/call": {
      const isStatus = params.name === "status";
      const text = isStatus ? JSON.stringify(note.payload) : `${note.instructions}\n\n${note.fix}`;
      const result = { content: [{ type: "text", text }], isError: !isStatus };
      return ok(modern ? Object.assign({ resultType: "complete" }, result) : result);
    }
    case "resources/list":
      return listed({ resources: [] });
    case "resources/templates/list":
      return listed({ resourceTemplates: [] });
    case "prompts/list":
      return listed({ prompts: [] });
    default:
      return failure(id, -32601, `method not found: ${method}`);
  }
}

/** Serve the stub over newline-delimited JSON-RPC until `input` ends. */
function serveStub(note, input, output) {
  const lines = readline.createInterface({ input, crlfDelay: Infinity });
  lines.on("line", (line) => {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch (_) {
      output.write(`${JSON.stringify(failure(null, -32700, "parse error"))}\n`);
      return;
    }
    const reply = stubAnswer(message, note);
    if (reply) output.write(`${JSON.stringify(reply)}\n`);
  });
  lines.on("close", () => output.end());
}

const KILL_GRACE_MS = 2000;
const FORWARDED_SIGNALS = ["SIGTERM", "SIGINT", "SIGHUP"];

/**
 * Ask the child to stop with `signal`, and kill it for good when it is still
 * there after `graceMs`. The timer never keeps this process alive.
 */
function stopChild(child, signal = "SIGTERM", graceMs = KILL_GRACE_MS) {
  child.kill(signal);
  const timer = setTimeout(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  }, graceMs);
  timer.unref();
}

/**
 * Pass a stop signal this process gets on to the child. This process then
 * stays until the child has gone, so its last output is still relayed.
 * Windows has none of these signals to forward, so nothing is set up there.
 */
function forwardSignals(child, target = process, platform = process.platform, graceMs = KILL_GRACE_MS) {
  if (platform === "win32") return;
  for (const signal of FORWARDED_SIGNALS) {
    target.on(signal, () => stopChild(child, signal, graceMs));
  }
}

/**
 * Relay stdio byte for byte. When `input` ends, the child's stdin closes and
 * the child gets 5 s to leave before it is stopped; `exit` gets its code.
 * When `output` fails (Claude Desktop closed the pipe), the child is stopped
 * and the relay ends quietly.
 */
function relay(child, input, output, errors, exit) {
  child.stdin.on("error", () => {});
  output.on("error", () => stopChild(child));
  errors.on("error", () => {});
  input.pipe(child.stdin);
  child.stdout.pipe(output);
  child.stderr.pipe(errors);
  input.on("end", () => {
    const timer = setTimeout(() => stopChild(child), 5000);
    timer.unref();
  });
  child.on("close", (code) => exit(code === null ? 1 : code));
}

/**
 * Start `command args` with `env` and relay it to `io`. A child that cannot
 * start raises `error` and then `close`, so `io.exit` is guarded to run once.
 */
function start(command, args, env, io) {
  let exited = false;
  const exit = (code) => {
    if (exited) return;
    exited = true;
    io.exit(code);
  };
  const child = childProcess.spawn(command, args, {
    env,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  child.on("error", (e) => {
    io.errors.write(`crystalline extension: could not start ${command}: ${e.message}\n`);
    exit(1);
  });
  relay(child, io.input, io.output, io.errors, exit);
  return child;
}

/** This extension's version, from the manifest one folder up. */
function extensionVersion() {
  try {
    return JSON.parse(fs.readFileSync(path.join(__dirname, "..", "manifest.json"), "utf8")).version;
  } catch (_) {
    return MINIMUM_TEXT;
  }
}

function main() {
  // A write after Claude Desktop closed the pipe fails with EPIPE: end
  // quietly instead of with a stack trace.
  process.stdout.on("error", () => {});
  const outcome = findBinary({ platform: process.platform, env: process.env, isFile, runVersion });
  if (outcome.kind === "ok") {
    const child = start(outcome.path, ["mcp"], Object.assign({}, process.env, { CRYSTALLINE_CHANNEL: "desktop" }), {
      input: process.stdin,
      output: process.stdout,
      errors: process.stderr,
      // A piped stdout is asynchronous on macOS, so the child's last reply
      // may still sit in its buffer: flush it before the process ends.
      exit: (code) => process.stdout.write("", () => process.exit(code)),
    });
    forwardSignals(child);
    return;
  }
  const note = notice(outcome, process.platform, extensionVersion());
  process.stderr.write(`crystalline extension: ${note.payload.reason}\n`);
  serveStub(note, process.stdin, process.stdout);
}

module.exports = {
  MINIMUM,
  candidates,
  parseVersion,
  atLeast,
  findBinary,
  runVersion,
  notice,
  stubAnswer,
  serveStub,
  relay,
  stopChild,
  forwardSignals,
  start,
  main,
};

if (require.main === module) {
  main();
}
