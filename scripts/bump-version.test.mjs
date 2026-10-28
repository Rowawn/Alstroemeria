import test from "node:test";
import assert from "node:assert/strict";
import { setCargoLockVersion, setCargoTomlVersion, setTauriConf, versionCode } from "./bump-version.mjs";

test("versionCode follows major*10000 + minor*100 + patch", () => {
  assert.equal(versionCode("0.1.0"), 100);
  assert.equal(versionCode("0.1.1"), 101);
  assert.equal(versionCode("1.2.3"), 10203);
  assert.throws(() => versionCode("1.2"));
  assert.throws(() => versionCode("0.100.0"));
});

test("Cargo.toml: only the package version changes", () => {
  const t = '[package]\nname = "x"\nversion = "0.1.0"\n\n[dependencies]\ntauri = { version = "2" }\nserde = "1"\n';
  const out = setCargoTomlVersion(t, "0.2.0");
  assert.match(out, /^version = "0.2.0"$/m);
  assert.match(out, /tauri = \{ version = "2" \}/);
});

test("Cargo.lock: only alstroemeria changes", () => {
  const t = 'name = "other"\nversion = "0.1.0"\n\n[[package]]\nname = "alstroemeria"\nversion = "0.1.0"\n';
  const out = setCargoLockVersion(t, "0.3.0");
  assert.match(out, /name = "alstroemeria"\nversion = "0.3.0"/);
  assert.match(out, /name = "other"\nversion = "0.1.0"/);
});

test("tauri.conf.json gets version and versionCode", () => {
  const out = JSON.parse(setTauriConf('{"version":"0.1.0","bundle":{"active":true}}', "0.1.1"));
  assert.equal(out.version, "0.1.1");
  assert.equal(out.bundle.android.versionCode, 101);
  assert.equal(out.bundle.active, true);
});
