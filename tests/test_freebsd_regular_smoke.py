"""Regression coverage for the real normal-release FreeBSD gate."""
import ast
import importlib.util
from pathlib import Path
import textwrap

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("normal_freebsd_smoke", ROOT / "tests/test_freebsd_recovery_smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
smoke.WORKFLOW = ROOT / ".github/workflows/freebsd-release.yml"
smoke.SOURCE = textwrap.dedent(smoke.WORKFLOW.read_text().split("python3 - <<'PY'\n", 1)[1].split("            PY\n", 1)[0])
smoke.TREE = ast.parse(smoke.SOURCE)


class NormalReleaseSmokeTests(smoke.RecoverySmokeTests):
    def test_pinned_gpui_builds_without_a_dependency_overlay(self):
        source = smoke.WORKFLOW.read_text()
        self.assertNotIn("prepare-freebsd-gpui.py", source)
        build = source.index("bash jcode-desktop/scripts/package-freebsd.sh")
        self.assertLess(build, source.index("git -C jcode-desktop diff HEAD --exit-code"))
        self.assertIn('export CARGO_HOME="$HOME/.cargo-freebsd-release"', source)
        manifest = (ROOT / "Cargo.toml").read_text()
        self.assertIn('git = "https://github.com/1jehuang/gpui"', manifest)
