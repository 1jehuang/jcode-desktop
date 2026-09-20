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
    def test_dependency_overlay_before_build_and_verified_after(self):
        source = smoke.WORKFLOW.read_text()
        self.assertEqual(source.count("scripts/prepare-freebsd-gpui.py"), 2)
        prepare, verify = [source.index(line) for line in source.splitlines()
                           if "scripts/prepare-freebsd-gpui.py" in line]
        build = source.index("bash jcode-desktop/scripts/package-freebsd.sh")
        self.assertLess(prepare, build)
        self.assertLess(build, verify)
        self.assertIn("--verify-only", source[verify:])
        self.assertIn('export CARGO_HOME="$HOME/.cargo-freebsd-release"', source)
        self.assertEqual(source.count("git -C jcode-desktop diff HEAD --exit-code"), 2)
