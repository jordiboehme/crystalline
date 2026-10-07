#!/usr/bin/env node
"use strict";
// Stands in for a `crystalline --version` that hangs: it prints a valid
// version line, but only after 3 s, so a version check with a shorter
// timeout must give up before the line arrives.
setTimeout(() => process.stdout.write("crystalline 0.24.0\n"), 3000);
