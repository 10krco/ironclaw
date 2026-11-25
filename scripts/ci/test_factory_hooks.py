#!/usr/bin/env python3
"""Check Git hook migration and distinguish task-board pushes from source pushes."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
LEGACY_HOOKS = {
    "commit-msg": "scripts/commit-msg-regression.sh",
    "pre-commit": "scripts/pre-commit-safety.sh",
    "pre-push": ".githooks/pre-push",
}


class FactoryHooksTest(unittest.TestCase):
    def setUp(self):
        workspace = tempfile.TemporaryDirectory(prefix="ironclaw-hooks-")
        self.addCleanup(workspace.cleanup)
        self.workspace = Path(workspace.name)
        self.repo = self.workspace / "repo"
        self.git("init", "--quiet", "-b", "main", str(self.repo), cwd=self.workspace)
        self.hooks = self.repo / ".git/hooks"
        self.env = dict(
            os.environ,
            PATH=f"{self.workspace}{os.pathsep}{os.environ['PATH']}",
            IRONCLAW_PREPUSH_MERGE_CHECK="1",
        )

    def git(self, *args, cwd=None):
        return subprocess.run(
            ["git", "-c", "commit.gpgsign=false", "-c", "user.name=Probe",
             "-c", "user.email=probe@example.org", *args],
            cwd=cwd or self.repo, env=getattr(self, "env", None),
            check=True, capture_output=True, text=True, timeout=20,
        )

    def script(self, path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        path.chmod(0o755)

    def test_migration_restores_previous_hooks_after_partial_install(self):
        for hook, path in LEGACY_HOOKS.items():
            target = self.repo / path
            self.script(target, "#!/bin/sh\nexit 0\n")
            (self.hooks / hook).symlink_to(target)
        self.script(
            self.workspace / "lefthook",
            "#!/bin/sh\n"
            "if [ \"$#\" -eq 1 ]; then printf 'local hook\\n' > .git/hooks/post-checkout; fi\n"
            "printf 'partial replacement\\n' > .git/hooks/pre-commit\nexit 53\n",
        )
        setup = (ROOT / "scripts/dev-setup.sh").read_text()
        stage = setup.split("# 6. Install git hooks\n", 1)[1].split("\necho \"\"\n# Codebase", 1)[0]

        def run_setup(cwd=None):
            return subprocess.run(
                ["bash", "--noprofile", "--norc", "-euc", stage],
                cwd=cwd or self.repo, env=self.env, capture_output=True, text=True, timeout=20,
            )

        self.assertEqual(run_setup().returncode, 53)
        self.assertFalse((self.hooks / "post-checkout").exists())
        for hook, path in LEGACY_HOOKS.items():
            self.assertEqual((self.hooks / hook).readlink(), self.repo / path)

            self.assertEqual((self.repo / path).read_text(), "#!/bin/sh\nexit 0\n")

        replacement = self.hooks / "pre-push"
        replacement.unlink()
        self.script(replacement, "#!/bin/sh\n# Existing custom policy\nexit 0\n")
        self.script(
            self.workspace / "lefthook",
            "#!/bin/sh\nmv .git/hooks/pre-push .git/hooks/pre-push.old\n"
            "printf 'partial replacement\\n' > .git/hooks/pre-push\nexit 53\n",
        )
        self.assertEqual(run_setup().returncode, 53)
        self.assertIn("# Existing custom policy", replacement.read_text())
        self.assertFalse((self.hooks / "pre-push.old").exists())

        replacement.unlink()
        replacement.symlink_to(self.repo / LEGACY_HOOKS["pre-push"])
        self.git("config", "core.hooksPath", ".git/hooks")
        result = run_setup()
        self.assertEqual(result.returncode, 1)
        self.assertIn("configured core.hooksPath", result.stderr)
        for hook, path in LEGACY_HOOKS.items():
            self.assertEqual((self.hooks / hook).readlink(), self.repo / path)

        self.git("config", "--unset", "core.hooksPath")
        self.git("commit", "--allow-empty", "-m", "test worktree baseline")
        linked = self.workspace / "linked"
        self.git("worktree", "add", "-b", "linked", str(linked))
        self.script(self.workspace / "lefthook", "#!/bin/sh\nexit 53\n")
        result = run_setup(linked)
        self.assertEqual(result.returncode, 53, result.stderr)
        for hook, path in LEGACY_HOOKS.items():
            self.assertEqual((self.hooks / hook).readlink(), self.repo / path)

            (self.hooks / hook).unlink()
        self.script(self.workspace / "lefthook", "#!/bin/sh\nprintf 'clean install reached\\n'\n")
        clean = run_setup()
        self.assertEqual(clean.returncode, 0, clean.stderr)
        self.assertIn("clean install reached", clean.stdout)

    def test_only_tiber_updates_skip_source_gates(self):
        if not shutil.which("lefthook"):
            self.skipTest("Lefthook is needed to exercise actual Git hook dispatch")
        remote = self.workspace / "origin.git"
        self.git("init", "--quiet", "--bare", str(remote))
        self.git("remote", "add", "origin", str(remote))
        probe = self.repo / "probe.txt"
        probe.write_text("base\n")
        self.git("add", "probe.txt")
        self.git("commit", "--quiet", "-m", "test base")
        self.git("push", "--quiet", "origin", "main")
        probe.write_text("changed\n")
        self.git("add", "probe.txt")
        self.git("commit", "--quiet", "-m", "test update")

        (self.repo / ".githooks").mkdir()
        for path in ("lefthook.yml", ".lefthookrc", ".githooks/pre-push"):
            shutil.copy2(ROOT / path, self.repo / path)
        calls = self.workspace / "calls"
        self.env["PROBE_CHECKS"] = str(calls)
        for path in (
            self.workspace / "cargo",
            self.repo / "scripts/ci/check-include-str-paths.sh",
            self.repo / "scripts/ci/check-hermetic-env.sh",
        ):
            self.script(path, "#!/bin/sh\nprintf 'check\\n' >> \"$PROBE_CHECKS\"\n")
        subprocess.run(
            ["lefthook", "install"], cwd=self.repo, env=self.env,
            check=True, capture_output=True, timeout=20,
        )

        tiber_ref = "HEAD:refs/heads/tiber"
        self.git("push", "--dry-run", "origin", "HEAD~1:refs/heads/tiber")
        self.assertFalse(calls.exists())
        self.git("push", "--dry-run", "origin", tiber_ref)
        self.assertFalse(calls.exists())
        self.git("push", "--dry-run", "origin", tiber_ref, "HEAD:refs/heads/probe")
        self.assertEqual(calls.read_text().splitlines(), ["check"] * 3)
        calls.unlink()
        self.git("push", "--quiet", "origin", tiber_ref)
        self.git("push", "--dry-run", "origin", ":refs/heads/tiber")
        self.assertEqual(calls.read_text().splitlines(), ["check"] * 3)

        oid = self.git("rev-parse", "HEAD").stdout.strip()
        board_update = f"HEAD {oid} refs/heads/tiber {'0' * len(oid)}\n"
        for record in (
            board_update.rsplit(" ", 1)[0] + "\n",
            board_update + "HEAD",
            board_update.rstrip("\n") + "\t\n",
            board_update.replace("HEAD ", "HEAD  ", 1),
            board_update.replace("refs/heads/tiber", "refs/heads/ti\x00ber"),
            board_update.replace("HEAD ", "refs/heads/invalid..branch ", 1),
        ):
            with self.subTest(record=record):
                calls.unlink(missing_ok=True)
                subprocess.run(
                    ["./.githooks/pre-push", "origin", "--fast", "--git-hook"],
                    input=record, cwd=self.repo, env=self.env, text=True,
                    check=True, capture_output=True, timeout=20,
                )
                self.assertEqual(calls.read_text().splitlines(), ["check"] * 3)

        calls.unlink()
        self.git("remote", "rename", "origin", "review's")
        self.git("push", "--dry-run", "review's", "HEAD:refs/heads/probe")
        self.assertEqual(calls.read_text().splitlines(), ["check"] * 3)

    def test_commit_message_path_in_linked_worktree(self):
        if not shutil.which("lefthook"):
            self.skipTest("Lefthook is needed to exercise actual Git hook dispatch")
        self.git("commit", "--allow-empty", "-m", "test base")
        worktree = self.workspace / "review$IRONCLAW_HOOK_PROBE-worktree"
        self.git("worktree", "add", "-b", "probe", str(worktree))
        for path in ("lefthook.yml", ".lefthookrc"):
            shutil.copy2(ROOT / path, worktree / path)
        for path in (".githooks/pre-commit", "scripts/pre-commit-safety.sh"):
            self.script(worktree / path, "#!/bin/sh\nexit 0\n")
        self.script(
            worktree / "scripts/commit-msg-regression.sh",
            '#!/bin/sh\nset -eu\ngrep -Fxq "chore: verify argument" "$1"\nprintf \'%s\\n\' "$1" > "$PROBE_COMMIT_MSG"\n',
        )
        message_argument = self.workspace / "message-argument"
        self.env["PROBE_COMMIT_MSG"] = str(message_argument)
        subprocess.run(
            ["lefthook", "install"], cwd=worktree, env=self.env,
            check=True, capture_output=True, timeout=20,
        )
        self.git("commit", "--allow-empty", "-m", "chore: verify argument", cwd=worktree)
        self.assertIn(worktree.name, message_argument.read_text())

    def test_wit_version_check_uses_staged_contents(self):
        wasm = self.repo / "crates/lanes/ironclaw_wasm"
        wit = wasm / "wit/channel.wit"
        wit.parent.mkdir(parents=True)
        wit.write_text("package near:channel@1.0.0;\n")
        (wasm / "Cargo.toml").write_text('[package]\nname = "ironclaw_wasm"\n')
        self.git("add", str(wasm))
        self.git("commit", "-m", "test baseline")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.script(
            self.repo / "scripts/ci/crate-dir.sh",
            '#!/bin/sh\nfind crates -name Cargo.toml -path "*/$1/Cargo.toml" -exec dirname {} \\;\n',
        )
        for path in ("scripts/check-version-bumps.sh", ".githooks/pre-commit"):
            self.script(self.repo / path, (ROOT / path).read_text())

        hook = self.repo / ".githooks/pre-commit"
        wit.write_text("package near:channel@1.0.0;\n// ABI change\n")
        self.git("add", str(wit))
        wit.write_text("package near:channel@2.0.0;\n// ABI change\n")
        env = dict(self.env, GITHUB_BASE_REF="main", ALLOW_SKIP_VERSION_CHECK="false")
        rejected = subprocess.run([hook], cwd=self.repo, env=env, capture_output=True, text=True, timeout=20)
        self.assertEqual(rejected.returncode, 1, rejected.stdout)
        self.assertIn("package version was not bumped", rejected.stdout)

        other_family = self.repo / "crates/other"
        other_family.mkdir()
        moved = wasm.rename(other_family / wasm.name)
        rejected = subprocess.run([hook], cwd=self.repo, env=env, capture_output=True, text=True, timeout=20)
        self.assertEqual(rejected.returncode, 1, rejected.stdout)
        self.assertIn("package version was not bumped", rejected.stdout)
        moved.rename(wasm)

        self.git("add", str(wit))
        wit.write_text("package near:channel@1.0.0;\n")
        accepted = subprocess.run([hook], cwd=self.repo, env=env, capture_output=True, text=True, timeout=20)
        self.assertEqual(accepted.returncode, 0, accepted.stdout)
        self.assertIn("package version bumped", accepted.stdout)

        self.git("update-ref", "-d", "refs/remotes/origin/main")
        missing_base = subprocess.run([hook], cwd=self.repo, env=env, capture_output=True, text=True, timeout=20)
        self.assertEqual(missing_base.returncode, 1, missing_base.stdout)
        self.assertIn("fetch it before running the staged version check", missing_base.stderr)
        self.assertNotIn("Fetching origin/", missing_base.stdout)


if __name__ == "__main__":
    unittest.main()
