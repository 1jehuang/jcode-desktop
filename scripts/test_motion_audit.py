#!/usr/bin/env python3
"""Hermetic tests: no compiler, running app, display, network or git required."""
import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest

import motion_audit as audit


METHOD = 'fn toggle_drawer(&mut self, cx: &mut Context<Self>) { self.open = !self.open; cx.notify(); }'


class MotionAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / audit.SOURCE / 'view.rs'
        self.source.parent.mkdir(parents=True)
        self.source.write_text('impl View {\n' + METHOD + '\n}\n')

    def baseline(self):
        candidate = audit.scan(self.root)[0][0]
        evidence = {'kind': 'source', 'path': candidate['path'], 'symbol': candidate['symbol'],
                    'sha256': candidate['sha256'], 'note': 'Direct state change.'}
        return {'version': 1, 'scope': 'named-ui-methods-v1', 'decisions': [{
            'id': candidate['id'], 'sha256': candidate['sha256'], 'decision': 'gap',
            'reason': 'Drawer snaps. Add reversible transition.', 'priority': 1,
            'evidence': [evidence], 'test_gap': 'No motion test yet.'}]}

    def check_cli(self, inventory):
        path = self.root / audit.INVENTORY
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(inventory))
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return audit.main(['--root', str(self.root), '--check'])

    def test_known_gap_is_reviewed_not_check_failure(self):
        inventory = self.baseline()
        result = audit.audit(self.root, inventory)
        self.assertEqual(result['errors'], [])
        self.assertEqual(len(result['gaps']), 1)
        self.assertEqual(self.check_cli(inventory), 0)

    def test_new_named_method_fails_check(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text() + METHOD.replace('toggle_drawer', 'open_new_menu'))
        result = audit.audit(self.root, inventory)
        self.assertEqual([c['symbol'] for c in result['unreviewed']], ['open_new_menu'])
        self.assertEqual(self.check_cli(inventory), 1)

    def test_changed_reviewed_body_fails_check(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text().replace('!self.open', 'false'))
        result = audit.audit(self.root, inventory)
        self.assertTrue(any('stale source review' in error for error in result['errors']))
        self.assertEqual(self.check_cli(inventory), 1)

    def test_removed_or_renamed_candidate_is_stale(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text().replace('toggle_drawer', 'set_drawer'))
        self.assertTrue(any('stale inventory' in e for e in audit.audit(self.root, inventory)['errors']))

    def test_line_movement_does_not_invalidate_review(self):
        inventory = self.baseline()
        self.source.write_text('// unrelated heading\n\n' + self.source.read_text())
        self.assertEqual(audit.audit(self.root, inventory)['errors'], [])

    def test_changed_deleted_and_renamed_test_evidence_fail(self):
        for mutation in ('changed', 'deleted', 'renamed'):
            with self.subTest(mutation=mutation):
                inventory = self.baseline()
                test = self.source.with_name('view_tests.rs')
                test.write_text('fn drawer_reverses() { assert!(true); }')
                evidence = {'kind': 'test', 'path': test.relative_to(self.root).as_posix(),
                            'symbol': 'drawer_reverses', 'note': 'Motion regression.'}
                evidence['sha256'] = audit.evidence_hash(self.root, evidence)
                inventory['decisions'][0]['evidence'].append(evidence)
                self.assertEqual(self.check_cli(inventory), 0)
                if mutation == 'changed':
                    test.write_text('fn drawer_reverses() { assert!(false); }')
                elif mutation == 'deleted':
                    test.unlink()
                else:
                    test.write_text('fn renamed() { assert!(true); }')
                self.assertEqual(self.check_cli(inventory), 1)

    def test_whole_file_evidence_changes(self):
        inventory = self.baseline()
        test = self.root / 'acceptance.py'
        test.write_text('assert True\n')
        evidence = {'kind': 'test', 'path': 'acceptance.py', 'note': 'Acceptance script.'}
        evidence['sha256'] = audit.evidence_hash(self.root, evidence)
        inventory['decisions'][0]['evidence'].append(evidence)
        test.write_text('assert False\n')
        self.assertEqual(self.check_cli(inventory), 1)

    def test_comments_strings_raw_strings_chars_lifetimes(self):
        text = '''// fn open_fake(&mut self, cx: &mut Context<Self>) {}
/* nested /* } */ fn close_fake(&mut self, cx: &mut Context<Self>) {} */
const TEXT: &str = r###"fn toggle_fake(&mut self, cx: &mut Context<Self>) { }"###;
impl View {
 fn open_real<'a>(&'a mut self, cx: &mut Context<Self>) { let c = '}'; }
 fn toggle_drawer(&mut self, cx: &mut Context<Self>) {
   let s = "escaped \\\" }"; let raw = br#"}"#; let c = '\\'';
   self.open = true;
 }
}
'''
        self.source.write_text(text)
        candidates, _ = audit.scan(self.root)
        self.assertEqual([c['symbol'] for c in candidates], ['open_real', 'toggle_drawer'])
        fn = [f for f in audit.functions(text) if f['name'] == 'toggle_drawer'][0]
        self.assertIn('self.open = true;', text[fn['start']:fn['end']])

    def test_test_modules_and_files_are_excluded_without_truncating_production(self):
        self.source.write_text('#[cfg(test)] mod tests { impl V {' + METHOD + '} }\n' + METHOD.replace('toggle_drawer', 'open_real'))
        self.source.with_name('view_tests.rs').write_text(METHOD)
        self.assertEqual([c['symbol'] for c in audit.scan(self.root)[0]], ['open_real'])

    def test_handlers_and_conditionals_are_advisory_not_gated(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text() + '\nfn render() { div().on_click(|_| {}).when(self.open, |d| d); if self.open {} }')
        result = audit.audit(self.root, inventory)
        self.assertEqual(result['errors'], [])
        self.assertEqual([s['kind'] for s in result['advisory']].count('handler'), 1)
        self.assertEqual([s['kind'] for s in result['advisory']].count('conditional'), 2)

    def test_selected_conditional_enclosing_function_is_gated(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text() + '\nfn render(&self) { div().when(self.open, |d| d); }')
        inventory['additional_sites'] = [{'path': self.source.relative_to(self.root).as_posix(), 'symbol': 'render'}]
        result = audit.audit(self.root, inventory)
        self.assertEqual([c['symbol'] for c in result['unreviewed']], ['render'])

    def test_missing_selected_site_fails(self):
        inventory = self.baseline()
        inventory['additional_sites'] = [{'path': 'missing.rs', 'symbol': 'render'}]
        self.assertEqual(self.check_cli(inventory), 1)

    def test_duplicate_or_invalid_inventory_fails(self):
        for field, value in [('reason', ''), ('decision', 'covered'), ('evidence', []), ('priority', 0), ('test_gap', '')]:
            with self.subTest(field=field):
                inventory = self.baseline()
                inventory['decisions'][0][field] = value
                self.assertEqual(self.check_cli(inventory), 1)
        inventory = self.baseline()
        inventory['decisions'].append(copy.deepcopy(inventory['decisions'][0]))
        self.assertEqual(self.check_cli(inventory), 1)

    def test_ambiguous_symbols_fail_closed(self):
        inventory = self.baseline()
        self.source.write_text(self.source.read_text() + '\nimpl Other {' + METHOD + '}')
        self.assertEqual(self.check_cli(inventory), 1)

    def test_evidence_cannot_escape_root(self):
        with self.assertRaisesRegex(ValueError, 'escapes repository'):
            audit.evidence_hash(self.root, {'path': '../outside.rs'})

    def test_bad_root_and_malformed_inventory_fail_closed(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(audit.main(['--root', str(self.root / 'missing'), '--check']), 2)
        self.assertEqual(self.check_cli({'decisions': None}), 2)

    def test_report_is_deterministic(self):
        inventory = self.baseline()
        self.assertEqual(json.dumps(audit.audit(self.root, inventory)), json.dumps(audit.audit(self.root, inventory)))


if __name__ == '__main__':
    unittest.main()
