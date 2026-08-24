import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  PROFILE_NAMES,
  bundled,
  copyAll,
  parseArgs,
  profileBundle,
  readManifest,
  workflowStatus,
} from "../bin/install.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(TEST_DIR, "../../..");

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-installer-"));
  const home = path.join(root, "home");
  const share = path.join(root, "share");
  fs.mkdirSync(path.join(share, "agents"), { recursive: true });
  fs.mkdirSync(path.join(share, "skills", "alpha"), { recursive: true });
  fs.writeFileSync(path.join(share, "agents", "mastermind-one.md"), "one\n");
  fs.writeFileSync(path.join(share, "skills", "alpha", "SKILL.md"), "alpha\n");
  return {
    root,
    home,
    share,
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

function findSkillDirs(root) {
  const found = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const directory = path.join(root, entry.name);
    if (fs.existsSync(path.join(directory, "SKILL.md"))) found.push(directory);
    else found.push(...findSkillDirs(directory));
  }
  return found;
}

function completeFixture() {
  const f = fixture();
  fs.rmSync(f.share, { recursive: true, force: true });
  fs.mkdirSync(path.join(f.share, "agents"), { recursive: true });
  fs.mkdirSync(path.join(f.share, "skills"), { recursive: true });
  for (const entry of fs.readdirSync(path.join(REPO_ROOT, "agents", "subagents"))) {
    if (!entry.endsWith(".md")) continue;
    fs.copyFileSync(
      path.join(REPO_ROOT, "agents", "subagents", entry),
      path.join(f.share, "agents", entry),
    );
  }
  for (const source of findSkillDirs(path.join(REPO_ROOT, "skills"))) {
    fs.cpSync(source, path.join(f.share, "skills", path.basename(source)), {
      recursive: true,
    });
  }
  return f;
}

function manifestPath(f, client) {
  return path.join(f.home, client === "claude" ? ".claude" : ".codex", ".mastermind-workflow.json");
}

test("update reconciles owned artifacts and preserves unrelated client files", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "claude", version: "1.0.0", profile: "full" });
    const claude = path.join(f.home, ".claude");
    fs.writeFileSync(path.join(claude, "skills", "alpha", "retired.txt"), "stale\n");
    fs.mkdirSync(path.join(claude, "skills", "user-skill"), { recursive: true });
    fs.writeFileSync(path.join(claude, "skills", "user-skill", "SKILL.md"), "user\n");
    fs.writeFileSync(path.join(claude, "agents", "user-agent.md"), "user\n");

    fs.rmSync(path.join(f.share, "agents", "mastermind-one.md"));
    fs.writeFileSync(path.join(f.share, "agents", "mastermind-two.md"), "two\n");
    fs.writeFileSync(path.join(f.share, "skills", "alpha", "SKILL.md"), "alpha v2\n");
    copyAll({ home: f.home, share: f.share, client: "claude", version: "2.0.0" });

    assert.equal(fs.existsSync(path.join(claude, "agents", "mastermind-one.md")), false);
    assert.equal(fs.readFileSync(path.join(claude, "agents", "mastermind-two.md"), "utf8"), "two\n");
    assert.equal(fs.existsSync(path.join(claude, "skills", "alpha", "retired.txt")), false);
    assert.equal(fs.readFileSync(path.join(claude, "skills", "alpha", "SKILL.md"), "utf8"), "alpha v2\n");
    assert.equal(fs.readFileSync(path.join(claude, "skills", "user-skill", "SKILL.md"), "utf8"), "user\n");
    assert.equal(fs.readFileSync(path.join(claude, "agents", "user-agent.md"), "utf8"), "user\n");

    const manifest = readManifest(path.join(claude, ".mastermind-workflow.json"));
    assert.deepEqual(manifest.artifacts.agents, ["mastermind-two.md"]);
    assert.deepEqual(manifest.artifacts.skills, ["alpha"]);
    assert.equal(manifest.schema_version, 2);
    assert.equal(manifest.profile, "full");
    assert.match(manifest.digests["skills/alpha"], /^[0-9a-f]{64}$/);
    assert.equal(workflowStatus({ home: f.home, share: f.share, client: "claude", version: "2.0.0" })[0].parity, true);
  } finally {
    f.cleanup();
  }
});

test("Codex adapter installs skills without Claude subagents", () => {
  const f = fixture();
  try {
    const [result] = copyAll({ home: f.home, share: f.share, client: "codex", version: "1.0.0", profile: "full" });
    assert.equal(result.subagents, 0);
    assert.equal(fs.existsSync(path.join(f.home, ".codex", "skills", "alpha", "SKILL.md")), true);
    assert.equal(fs.existsSync(path.join(f.home, ".codex", "agents")), false);
    assert.equal(workflowStatus({ home: f.home, share: f.share, client: "codex", version: "1.0.0" })[0].parity, true);
  } finally {
    f.cleanup();
  }
});

test("invalid ownership manifest fails before replacing installed files", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "claude", version: "1.0.0", profile: "full" });
    const claude = path.join(f.home, ".claude");
    fs.writeFileSync(path.join(claude, ".mastermind-workflow.json"), "not json\n");
    fs.writeFileSync(path.join(f.share, "agents", "mastermind-one.md"), "replacement\n");
    assert.throws(
      () => copyAll({ home: f.home, share: f.share, client: "claude", version: "2.0.0" }),
      /invalid workflow manifest/,
    );
    assert.equal(fs.readFileSync(path.join(claude, "agents", "mastermind-one.md"), "utf8"), "one\n");
  } finally {
    f.cleanup();
  }
});

test("newer manifest schemas fail before replacing installed files", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "claude", version: "1.0.0", profile: "full" });
    const claude = path.join(f.home, ".claude");
    const manifest = JSON.parse(
      fs.readFileSync(path.join(claude, ".mastermind-workflow.json"), "utf8"),
    );
    manifest.schema_version = 3;
    fs.writeFileSync(
      path.join(claude, ".mastermind-workflow.json"),
      `${JSON.stringify(manifest, null, 2)}\n`,
    );
    fs.writeFileSync(path.join(f.share, "agents", "mastermind-one.md"), "replacement\n");

    assert.throws(
      () => copyAll({ home: f.home, share: f.share, client: "claude", version: "2.0.0" }),
      /unsupported workflow manifest/,
    );
    assert.equal(fs.readFileSync(path.join(claude, "agents", "mastermind-one.md"), "utf8"), "one\n");
  } finally {
    f.cleanup();
  }
});

test("doctor detects installed content tampering", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "codex", version: "1.0.0", profile: "full" });
    fs.writeFileSync(
      path.join(f.home, ".codex", "skills", "alpha", "SKILL.md"),
      "tampered\n",
    );
    const [status] = workflowStatus({
      home: f.home,
      share: f.share,
      client: "codex",
      version: "1.0.0",
    });
    assert.equal(status.parity, false);
    assert.deepEqual(status.drifted, ["skills/alpha"]);
  } finally {
    f.cleanup();
  }
});

test("workflow profiles are bounded and closed over linked skills", () => {
  const f = completeFixture();
  try {
    const bundle = bundled(f.share);
    const profiles = Object.fromEntries(
      PROFILE_NAMES.map((profile) => [profile, profileBundle(bundle, profile, f.share)]),
    );

    assert.deepEqual(PROFILE_NAMES, ["core", "frontend", "security", "full"]);
    assert.equal(bundle.skills.length, 26);
    assert.equal(profiles.core.skills.length, 14);
    assert.equal(profiles.frontend.skills.length, 19);
    assert.equal(profiles.security.skills.length, 17);
    assert.equal(profiles.full.skills.length, 26);
    assert.deepEqual(profiles.full.skills, bundle.skills);
    assert.deepEqual(profiles.frontend.subagents, bundle.subagents);
    assert.deepEqual(profiles.security.subagents, bundle.subagents);
    for (const skill of profiles.core.skills) {
      assert.equal(profiles.frontend.skills.includes(skill), true, skill);
      assert.equal(profiles.security.skills.includes(skill), true, skill);
    }

    fs.appendFileSync(
      path.join(f.share, "skills", "mastermind-project-map", "SKILL.md"),
      "\n[[mastermind-prompt-refiner]]\n",
    );
    assert.throws(
      () => profileBundle(bundle, "core", f.share),
      /profile core is not closed.*mastermind-prompt-refiner/,
    );
  } finally {
    f.cleanup();
  }
});

test("fresh installs default to core and doctor resolves the installed profile", () => {
  const f = completeFixture();
  try {
    const [result] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
    });
    const manifest = readManifest(manifestPath(f, "claude"));
    const [status] = workflowStatus({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
    });

    assert.equal(result.profile, "core");
    assert.equal(result.skills, 14);
    assert.equal(manifest.schema_version, 2);
    assert.equal(manifest.profile, "core");
    assert.equal(manifest.artifacts.skills.length, 14);
    assert.equal(
      fs.existsSync(path.join(f.home, ".claude", "skills", "mastermind-prompt-refiner")),
      false,
    );
    assert.equal(status.profile, "core");
    assert.equal(status.parity, true);
  } finally {
    f.cleanup();
  }
});

test("legacy manifests migrate as full without silently dropping skills", () => {
  const f = completeFixture();
  try {
    copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
      profile: "full",
    });
    const current = readManifest(manifestPath(f, "claude"));
    const { profile: _profile, ...legacy } = current;
    legacy.schema_version = 1;
    fs.writeFileSync(manifestPath(f, "claude"), `${JSON.stringify(legacy, null, 2)}\n`);

    const [result] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "2.0.0",
    });
    const migrated = readManifest(manifestPath(f, "claude"));

    assert.equal(result.profile, "full");
    assert.equal(result.skills, 26);
    assert.equal(migrated.schema_version, 2);
    assert.equal(migrated.profile, "full");
    assert.equal(migrated.artifacts.skills.length, 26);
  } finally {
    f.cleanup();
  }
});

test("explicit profile switches reconcile owned skills and preserve user files", () => {
  const f = completeFixture();
  try {
    copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
      profile: "full",
    });
    const userSkill = path.join(f.home, ".claude", "skills", "user-skill");
    fs.mkdirSync(userSkill, { recursive: true });
    fs.writeFileSync(path.join(userSkill, "SKILL.md"), "user\n");

    const [core] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "2.0.0",
      profile: "core",
    });
    assert.equal(core.skills, 14);
    assert.equal(
      fs.existsSync(path.join(f.home, ".claude", "skills", "mastermind-prompt-refiner")),
      false,
    );
    assert.equal(fs.readFileSync(path.join(userSkill, "SKILL.md"), "utf8"), "user\n");

    const [frontend] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "3.0.0",
      profile: "frontend",
    });
    assert.equal(frontend.skills, 19);
    assert.equal(
      fs.existsSync(
        path.join(f.home, ".claude", "skills", "mastermind-browser-verification"),
      ),
      true,
    );

    const [security] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "4.0.0",
      profile: "security",
    });
    assert.equal(security.skills, 17);
    assert.equal(readManifest(manifestPath(f, "claude")).profile, "security");
    assert.equal(
      fs.existsSync(
        path.join(f.home, ".claude", "skills", "mastermind-browser-verification"),
      ),
      false,
    );
    assert.equal(
      fs.existsSync(
        path.join(f.home, ".claude", "skills", "mastermind-agent-security-review"),
      ),
      true,
    );
    assert.equal(fs.readFileSync(path.join(userSkill, "SKILL.md"), "utf8"), "user\n");
  } finally {
    f.cleanup();
  }
});

test("client all preserves each installed profile when no override is given", () => {
  const f = completeFixture();
  try {
    copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
      profile: "frontend",
    });
    copyAll({
      home: f.home,
      share: f.share,
      client: "codex",
      version: "1.0.0",
      profile: "security",
    });

    const results = copyAll({
      home: f.home,
      share: f.share,
      client: "all",
      version: "2.0.0",
    });
    assert.deepEqual(
      results.map(({ client, profile, skills }) => ({ client, profile, skills })),
      [
        { client: "claude", profile: "frontend", skills: 19 },
        { client: "codex", profile: "security", skills: 17 },
      ],
    );
    assert.equal(
      workflowStatus({
        home: f.home,
        share: f.share,
        client: "all",
        version: "2.0.0",
      }).every((status) => status.parity),
      true,
    );
  } finally {
    f.cleanup();
  }
});

test("client all rolls back an earlier client when a later client fails", () => {
  const f = completeFixture();
  try {
    copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
      profile: "full",
    });
    const claude = path.join(f.home, ".claude");
    const userFile = path.join(claude, "skills", "user-skill", "SKILL.md");
    fs.mkdirSync(path.dirname(userFile), { recursive: true });
    fs.writeFileSync(userFile, "user\n");
    const beforeManifest = fs.readFileSync(manifestPath(f, "claude"), "utf8");
    const beforeAgent = fs.readFileSync(
      path.join(claude, "agents", "mastermind-researcher.md"),
      "utf8",
    );
    fs.writeFileSync(path.join(f.home, ".codex"), "blocks directory creation\n");

    assert.throws(
      () =>
        copyAll({
          home: f.home,
          share: f.share,
          client: "all",
          version: "2.0.0",
          profile: "core",
        }),
      /EEXIST|ENOTDIR/,
    );

    assert.equal(fs.readFileSync(manifestPath(f, "claude"), "utf8"), beforeManifest);
    assert.equal(
      fs.readFileSync(path.join(claude, "agents", "mastermind-researcher.md"), "utf8"),
      beforeAgent,
    );
    assert.equal(
      fs.existsSync(path.join(claude, "skills", "mastermind-prompt-refiner")),
      true,
    );
    assert.equal(fs.readFileSync(userFile, "utf8"), "user\n");
    assert.deepEqual(
      fs.readdirSync(claude).filter((name) => name.startsWith(".mastermind-stage-")),
      [],
    );
  } finally {
    f.cleanup();
  }
});

test("committed installs report recoverable cleanup failures without rolling back", () => {
  const f = completeFixture();
  const originalRmSync = fs.rmSync;
  try {
    copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "1.0.0",
      profile: "full",
    });
    let injected = false;
    fs.rmSync = (target, options) => {
      if (!injected && path.basename(String(target)).startsWith(".mastermind-stage-")) {
        injected = true;
        throw new Error("injected finalization failure");
      }
      return originalRmSync(target, options);
    };

    const [result] = copyAll({
      home: f.home,
      share: f.share,
      client: "claude",
      version: "2.0.0",
      profile: "core",
    });
    const manifest = readManifest(manifestPath(f, "claude"));

    assert.equal(injected, true);
    assert.equal(result.profile, "core");
    assert.match(result.cleanup_error, /injected finalization failure/);
    assert.equal(fs.existsSync(result.cleanup_pending), true);
    assert.equal(
      fs.existsSync(path.join(result.cleanup_pending, "backup", "manifest.json")),
      true,
    );
    assert.equal(manifest.version, "2.0.0");
    assert.equal(manifest.profile, "core");
  } finally {
    fs.rmSync = originalRmSync;
    f.cleanup();
  }
});

test("profile arguments are explicit and validated", () => {
  assert.deepEqual(parseArgs(["install"]), {
    command: "install",
    client: "claude",
    json: false,
    profile: null,
  });
  assert.deepEqual(parseArgs(["update", "--client", "all", "--profile", "frontend"]), {
    command: "update",
    client: "all",
    json: false,
    profile: "frontend",
  });
  assert.throws(() => parseArgs(["install", "--profile"]), /--profile requires/);
  assert.throws(() => parseArgs(["install", "--profile", "unknown"]), /unsupported workflow profile/);
});
