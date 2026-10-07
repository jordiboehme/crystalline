"use strict";
// Stands in for a `crystalline mcp` that does not leave when asked: it
// ignores SIGTERM and stays until it is killed.
process.on("SIGTERM", () => {});
setInterval(() => {}, 1000);
process.stdout.write("ready\n");
