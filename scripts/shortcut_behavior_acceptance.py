"""Additional native Super-key behavior checks for accept-navigation.py."""
import json
from pathlib import Path


def sessions_attached(state, sessions):
    if not state:
        return False
    panels = state['rows'][0]['panels']
    return ([p['session'] for p in panels] == list(sessions)
            and all(p['history_loaded'] for p in panels))


def verify(key, check, wait, read_state, root: Path, generation, sessions):
    prefix = f'g{generation}-extended'
    original = list(sessions)
    last = len(original) - 1

    def attached():
        state = read_state()
        return state if sessions_attached(state, original) else False
    wait(attached, f'{prefix}-all-session-attachments-restored', 30)
    restored = check(f'{prefix}-attachments-restored', 0, 0, original)
    assert all(p['history_loaded'] for p in restored['rows'][0]['panels'])

    def step(chord, name, row=0, position=0, order=None):
        key(chord)
        return check(f'{prefix}-{name}', row, position,
                     original if order is None else order)

    step('super+Right', 'right-arrow', position=1)
    step('super+Left', 'left-arrow')
    step('super+Down', 'down-arrow', row=1, position=None, order=[])
    step('super+Up', 'up-arrow')
    step('super+End', 'end', position=last)
    step('super+Home', 'home')
    swapped = [original[1], original[0], *original[2:]]
    step('super+shift+l', 'move-right', position=1, order=swapped)
    step('super+shift+h', 'move-left')
    rotated = [*original[1:], original[0]]
    step('super+shift+End', 'move-last', position=last, order=rotated)
    step('super+shift+Home', 'move-first')
    moved = step('super+shift+j', 'move-down', row=1, order=[original[0]])
    assert [p['session'] for p in moved['rows'][0]['panels']] == original[1:]
    step('super+shift+k', 'move-up')
    key('super+shift+Tab')
    def overview_ready():
        state = read_state()
        return state if state and state['overview'] and state['keyboard_panel'] is None else False
    overview = wait(overview_ready, f'{prefix}-overview-alias-open', 10)
    assert overview['active_row'] == 0
    assert [p['session'] for p in overview['rows'][0]['panels']] == original
    assert overview['focused_slot'] == overview['rows'][0]['panels'][0]['slot']
    with (root / 'navigation.jsonl').open('a') as trace:
        trace.write(json.dumps({'checkpoint': f'{prefix}-overview-alias-open', 'state': overview}) + '\n')
    step('super+shift+Tab', 'overview-alias-close')

    def width(chord, expected, name):
        key(chord)
        def ready():
            state = read_state()
            if not state:
                return False
            panel = next((p for p in state['rows'][0]['panels']
                          if p['session'] == original[0]), None)
            return state if panel and abs(panel['width'] - expected) < .001 else False
        wait(ready, f'{prefix}-{name}-width', 10)
        state = check(f'{prefix}-{name}', 0, 0, original)
        assert abs(state['rows'][0]['panels'][0]['width'] - expected) < .001

    for number, expected in enumerate([.25, .5, .75, 1.0], 1):
        width(f'super+{number}', expected, f'width-{number}')
    width('super+r', .25, 'cycle-wrap')
    width('super+f', 1.0, 'maximize')
    width('super+f', .25, 'restore')
    width('super+2', .5, 'restore-default-width')

    key('super+n')
    def created():
        state = read_state()
        if not state:
            return False
        panels = state['rows'][0]['panels']
        if len(panels) != len(original) + 1:
            return False
        added = [p for p in panels if p['session'] not in original]
        return added[0] if len(added) == 1 and added[0]['session'].startswith('session_') else False
    added = wait(created, 'Super+N creates one real session', 30)
    order = [original[0], added['session'], *original[1:]]
    check(f'{prefix}-new-home-panel', 0, 1, order)

    def creation_record():
        for log in (root / 'jcode/logs').glob('jcode-*.log'):
            for line in log.read_text().splitlines():
                if 'ENV_SNAPSHOT ' in line:
                    record = json.loads(line.split('ENV_SNAPSHOT ', 1)[1])
                    if record.get('reason') == 'create' and record.get('session_id') == added['session']:
                        return record
        return None
    record = wait(creation_record, 'Super+N actual daemon directory', 10)
    assert record['working_dir'] == str(root / 'home'), record
    (root / f'extended-home-creation-g{generation}.json').write_text(json.dumps(record, indent=2) + '\n')
    step('super+q', 'close-added-home-panel', position=1)
    step('super+Home', 'restore-focus')

    for chord, session_id, name in [
        ('super+shift+g', 'gmail://inbox', 'gmail'),
        ('super+shift+d', 'todoist://tasks', 'todoist'),
        ('super+t', 'terminal', 'terminal'),
    ]:
        key(chord)
        order = [original[0], session_id, *original[1:]]
        check(f'{prefix}-open-{name}', 0, 1, order)
        if name != 'terminal':
            step('super+Home', f'{name}-away', order=order)
            step(chord, f'{name}-reuse', position=1, order=order)
        step('super+q', f'close-{name}', position=1)
        step('super+Home', f'after-{name}')

    parent_was_persisted = (root / 'jcode/sessions' / (original[0] + '.json')).exists()
    assert not parent_was_persisted, 'fresh empty parent must still exercise the unsaved-session path'
    key('super+space')
    forked = wait(created, 'Super+Space forks into one real session', 30)
    order = [original[0], forked['session'], *original[1:]]
    check(f'{prefix}-fork-panel', 0, 1, order)
    child_path = root / 'jcode/sessions' / (forked['session'] + '.json')
    wait(child_path.exists, 'forked session persisted', 10)
    child = json.loads(child_path.read_text())
    assert child['parent_id'] == original[0], child
    assert child['working_dir'] == str(root / 'home'), child
    assert child['messages'], 'fork notice should be persisted'
    (root / f'fork-lineage-g{generation}.json').write_text(json.dumps({
        'session_id': forked['session'], 'parent_id': child['parent_id'],
        'working_dir': child['working_dir'], 'parent_was_persisted': parent_was_persisted,
        'message_count': len(child['messages']),
    }, indent=2) + '\n')
    step('super+q', 'close-fork', position=1)
    step('super+Home', 'final-focus')
