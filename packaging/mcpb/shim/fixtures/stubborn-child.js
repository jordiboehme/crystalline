"use strict";
// Stands in for a `crystalline mcp` that does not leave when asked: it
// ignores SIGTERM and stays until it is killed. It ends itself after 10 s,
// far past any test's wait, so a hung run cannot leave it behind for good.
process.on("SIGTERM", () => {});
setInterval(() => {}, 1000);
setTimeout(() => process.exit(9), 10000).unref();
process.stdout.write("ready\n");
