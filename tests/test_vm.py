"""Unit tests for bin/vm — stdlib unittest only (the repo is stdlib-only, no pip).

The msb/tailscale subprocess boundary is mocked at the `vm.msb` / `vm.run` /
`vm.live_status` seams, so these run anywhere (no msb, no KVM, no tailnet).
The live end-to-end smoke test needs a KVM host and is manual (see the README /
CI notes), not part of this suite.
"""
import importlib.machinery
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
from contextlib import redirect_stdout
from types import SimpleNamespace
from unittest import mock

# bin/vm has no .py extension — load it explicitly via SourceFileLoader.
_REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_loader = importlib.machinery.SourceFileLoader("vm", os.path.join(_REPO, "bin", "vm"))
_spec = importlib.util.spec_from_loader("vm", _loader)
vm = importlib.util.module_from_spec(_spec)
_loader.exec_module(vm)


class PureHelperTests(unittest.TestCase):
    def test_parse_config_basic(self):
        cfg = vm.parse_config('image = "alpine"\ncpus = 2\n# c\nport=8080\n')
        self.assertEqual(cfg, {"image": "alpine", "cpus": 2, "port": 8080})

    def test_parse_config_ignores_sections_blanks_and_inline_comments(self):
        cfg = vm.parse_config('[defaults]\n\nmemory = "2G"  # cap\n')
        self.assertEqual(cfg, {"memory": "2G"})

    def test_split_ddash(self):
        self.assertEqual(vm.split_ddash(["exec", "web", "--", "ls", "-la"]),
                         (["exec", "web"], ["ls", "-la"]))
        self.assertEqual(vm.split_ddash(["ls"]), (["ls"], None))

    def test_serve_ports_in_use_parses_web_and_tcp(self):
        payload = json.dumps({"Web": {"nami.ts.net:8443": {}}, "TCP": {"10000": {}}})
        with mock.patch.object(vm, "TS", "tailscale"), \
                mock.patch.object(vm, "run", return_value=(0, payload, "")):
            self.assertEqual(vm.serve_ports_in_use(), {8443, 10000})


class _TmpState(unittest.TestCase):
    """Isolate all filesystem/DB state into a temp dir; mock the msb binary path."""
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self._patches = [
            mock.patch.object(vm, "STATE_DIR", self.tmp),
            mock.patch.object(vm, "DB_PATH", os.path.join(self.tmp, "state.db")),
            mock.patch.object(vm, "CONFIG_PATH", os.path.join(self.tmp, "config.toml")),
            mock.patch.object(vm, "MSB", "msb"),
            mock.patch.object(vm, "TS", None),
        ]
        for p in self._patches:
            p.start()

    def tearDown(self):
        for p in self._patches:
            p.stop()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _insert(self, **kw):
        con = vm.db()
        row = {"name": "web", "image": "python", "guest_port": 8000,
               "host_port": 40000, "created": "2026-01-01 00:00:00"}
        row.update(kw)
        con.execute(
            "INSERT INTO boxes(name,image,guest_port,host_port,created) VALUES(?,?,?,?,?)",
            (row["name"], row["image"], row["guest_port"], row["host_port"], row["created"]))
        con.commit()


class LsJsonTests(_TmpState):
    def test_empty_is_json_array(self):
        with mock.patch.object(vm, "live_status", return_value={}):
            buf = io.StringIO()
            with redirect_stdout(buf):
                vm.cmd_ls(SimpleNamespace(json=True))
        self.assertEqual(json.loads(buf.getvalue()), [])

    def test_box_shape(self):
        self._insert()
        with mock.patch.object(vm, "live_status", return_value={"web": "running"}):
            buf = io.StringIO()
            with redirect_stdout(buf):
                vm.cmd_ls(SimpleNamespace(json=True))
        data = json.loads(buf.getvalue())
        self.assertEqual(len(data), 1)
        self.assertEqual(data[0]["name"], "web")
        self.assertEqual(data[0]["status"], "running")
        self.assertIs(data[0]["public"], False)

    def test_json_strict_dies_on_msb_failure(self):
        # live_status(strict=True) must die when msb ls fails, not emit bogus rows.
        with mock.patch.object(vm, "msb", return_value=(1, "", "boom")):
            with self.assertRaises(SystemExit):
                vm.cmd_ls(SimpleNamespace(json=True))


class AllocServePortTests(_TmpState):
    def test_skips_used_port(self):
        con = vm.db()
        with mock.patch.object(vm, "serve_ports_in_use", return_value={8443}):
            self.assertEqual(vm.alloc_serve_port(con, public=False), 8444)

    def test_funnel_exhausted_dies(self):
        con = vm.db()
        with mock.patch.object(vm, "serve_ports_in_use", return_value={443, 8443, 10000}):
            with self.assertRaises(SystemExit):
                vm.alloc_serve_port(con, public=True)


class CmdNewTests(_TmpState):
    def _args(self, **kw):
        # Mirrors the `vm new` arg surface after all epics merged.
        base = dict(name=None, image=None, port=None, cpus=None, memory=None,
                    template=None, rebuild=False, no_persist=True, volume=None,
                    ttl=None, idle_timeout=None)
        base.update(kw)
        return SimpleNamespace(**base)

    def test_create_failure_exits_nonzero(self):
        with mock.patch.object(vm, "msb", return_value=(1, "", "boom")), \
                mock.patch.object(vm, "alloc_host_port", return_value=40000), \
                redirect_stdout(io.StringIO()):
            with self.assertRaises(SystemExit):
                vm.cmd_new(self._args())

    def test_config_default_image_applied(self):
        with open(os.path.join(self.tmp, "config.toml"), "w") as f:
            f.write('image = "alpine"\n')
        calls = []

        def fake_msb(*a, **k):
            calls.append(a)
            return (0, "", "")

        with mock.patch.object(vm, "msb", side_effect=fake_msb), \
                mock.patch.object(vm, "alloc_host_port", return_value=40000), \
                mock.patch.object(vm, "live_status", return_value={}), \
                redirect_stdout(io.StringIO()):
            vm.cmd_new(self._args(name="c"))
        # the create argv should carry the config image, not DEFAULT_IMAGE
        create = calls[0]
        self.assertIn("alpine", create)
        self.assertNotIn("python", create)


if __name__ == "__main__":
    unittest.main()
