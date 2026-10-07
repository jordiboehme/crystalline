#!/usr/bin/env node
"use strict";
// Stands in for a `crystalline --version` that fails: it prints a valid
// version line, then exits with code 2, so only the exit code can reject it.
process.stdout.write("crystalline 0.24.0\n");
process.exitCode = 2;
