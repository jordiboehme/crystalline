"use strict";
// Stands in for `crystalline mcp` in the relay test: copies stdin to stdout
// unchanged, writes a fixed line to stderr, and exits with code 3 once its
// stdin closes.
process.stderr.write("child stderr line\n");
process.stdin.on("data", (chunk) => process.stdout.write(chunk));
process.stdin.on("end", () => {
  process.stdout.end(() => process.exit(3));
});
