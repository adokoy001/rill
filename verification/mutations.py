"""Negative controls for the proofs. Edits exist only in a temporary copy of the policy."""
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASES = [
    ("forget to debit a received chunk", "*remaining -= accepted;", "*remaining -= 0;",
     "download_prefix_conserves_budget"),
    ("restore removed headers", "self.user_headers && same_origin", "same_origin",
     "every_accepted_step_preserves_invariants"),
    ("lose replay restriction", "(body || explicit_method_other_than_get_head)",
     "explicit_method_other_than_get_head", "every_accepted_step_preserves_invariants"),
    ("allow a downgrade", "self.https && !target_https", "false",
     "every_accepted_step_preserves_invariants"),
    ("saturate an overflowing byte counter", "used.checked_add(additional)",
     "Some(used.saturating_add(additional))", "budget_matches_mathematical_sum"),
    ("reject all redirects", "if self.https && !target_https", "if true",
     "all_and_only_permitted_steps_succeed"),
]


def main():
    original = (ROOT / "src/policy.rs").read_text()
    with tempfile.TemporaryDirectory() as folder:
        project = Path(folder)
        (project / "src").mkdir()
        (project / "verification").mkdir()
        shutil.copy(ROOT / "verification/check.rs", project / "verification/check.rs")
        for name, before, after, harness in CASES:
            if original.count(before) != 1:
                raise RuntimeError("mutation no longer applies: " + name)
            (project / "src/policy.rs").write_text(original.replace(before, after))
            proc = subprocess.run(["kani", "verification/check.rs", "--harness", harness,
                                   "--output-format", "terse"], cwd=project,
                                  text=True, capture_output=True, timeout=120)
            log = proc.stdout + proc.stderr
            if not (proc.returncode != 0 and "VERIFICATION:- FAILED" in log
                    and re.search(r"[1-9]\d* of \d+ failed", log)
                    and "Checking harness policy::" + harness in log):
                raise RuntimeError(f"{name}: expected an assertion failure\n{log}")
            print(f"detected: {name} [{harness}]")
    print(f"PASS {len(CASES)} negative controls")


if __name__ == "__main__":
    main()
