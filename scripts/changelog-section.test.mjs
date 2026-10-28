import test from "node:test";
import assert from "node:assert/strict";
import { section } from "./changelog-section.mjs";

const log = "# Changelog\n\n## [Unreleased]\n\n## [0.2.0] - 2026-11-01\n\n### Added\n- x\n\n## [0.1.0] - 2026-10-01\n\n- first\n";

test("extracts one version's section", () => {
  assert.equal(section(log, "0.2.0"), "### Added\n- x");
  assert.equal(section(log, "0.1.0"), "- first");
});

test("missing or empty sections are null", () => {
  assert.equal(section(log, "0.3.0"), null);
  assert.equal(section(log, "Unreleased"), null);
});
