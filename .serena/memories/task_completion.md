# Completing repository work

Before editing, inspect the worktree and preserve unrelated changes. Route the change to its owning family/crate and read its `AGENTS.md`, `README.md`, `CONTRACT.md`, plus relevant cross-crate contracts. Check repository task guidance when the work is tracked.

For a behavior change or fix, add or extend regression coverage first, run it to establish the expected failure, then implement. Prefer the existing test at the caller seam; production-wired behavior should be exercised through integration harnesses when that tier reaches the path. Update the owning contract/docs when behavior changes.

Before delivery, review all affected implementations/adapters when traits change, search siblings for the same bug pattern, check production code for forbidden panics and lost error causes, and run the narrowest meaningful checks plus architecture tests when crate placement or dependency edges change. Keep CI and docs consistent. See `mem:suggested_commands`, `mem:conventions`, and `mem:core`.