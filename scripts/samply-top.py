#!/usr/bin/env python3
"""Summarize a samply profile recorded with --unstable-presymbolicate.

    python3 scripts/samply-top.py profile.json.gz [--thread NAME] [--under SYMBOL] [--top N]

Prints self and inclusive sample counts per function for the busiest thread
(or the named one), optionally restricted to stacks containing SYMBOL.
"""
import argparse
import bisect
import collections
import gzip
import json
import subprocess
from pathlib import Path


def all_threads(profile):
    """Firefox profiles may place child-process threads in nested profiles."""
    for thread in profile.get('threads', []):
        yield thread, profile.get('libs', [])
    for process in profile.get('processes', []):
        yield from all_threads(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profile', type=Path)
    parser.add_argument('--thread')
    parser.add_argument('--under', help='only count stacks containing this substring')
    parser.add_argument('--top', type=int, default=40)
    parser.add_argument('--binary', action='append', default=[], metavar='LIBNAME=PATH',
                        help='symbolize a library the recorder could not read (a deleted or '
                             'replaced executable) from this copy of it, with nm')
    parser.add_argument('--threads', action='store_true', help='list threads by sample count')
    args = parser.parse_args()
    if args.top < 1:
        parser.error('--top must be positive')
    with (gzip.open(args.profile, 'rt') if args.profile.suffix == '.gz' else args.profile.open()) as source:
        profile = json.load(source)
    sidecar = Path(str(args.profile).removesuffix('.gz').removesuffix('.json') + '.json.syms.json')
    syms = json.loads(sidecar.read_text()) if sidecar.exists() else {'string_table': [], 'data': []}
    strings = syms['string_table']
    tables = {}
    for lib in syms['data']:
        table = sorted((s['rva'], s['size'], strings[s['symbol']]) for s in lib['symbol_table'])
        tables[(lib['debug_name'], lib.get('debug_id'))] = table
        tables.setdefault(lib['debug_name'], table)
    libs = [lib for _, libraries in all_threads(profile) for lib in libraries]
    for spec in args.binary:
        if '=' not in spec:
            parser.error('--binary requires LIBNAME=PATH')
        lib_name, path = spec.split('=', 1)
        # LIBNAME=PATH@0x1000 adds a bias, for recorders that report file offsets
        # where the text segment's address and file offset differ.
        path, _, bias = path.partition('@')
        bias = int(bias, 16) if bias else 0
        out = subprocess.run(['nm', '-C', '--defined-only', '-S', path], capture_output=True,
                             text=True, check=True).stdout
        table = []
        for line in out.splitlines():
            parts = line.split(None, 3)
            if len(parts) == 4 and parts[2] in 'tTwW':
                table.append((int(parts[0], 16) - bias, int(parts[1], 16), parts[3]))
        table.sort()
        for lib in libs:
            if lib_name in lib['name']:
                tables[lib['debugName']] = table
                tables[(lib['debugName'], lib.get('breakpadId'))] = table

    def resolve(lib_index, address):
        if lib_index is None or lib_index < 0 or lib_index >= len(libs):
            return hex(address) if address is not None and address >= 0 else '<unknown>'
        lib = libs[lib_index]
        table = tables.get((lib['debugName'], lib.get('breakpadId'))) or tables.get(lib['debugName'])
        if not table:
            return f"{lib['name']}+{address:#x}"
        i = bisect.bisect_right(table, (address, float('inf'), '')) - 1
        if i >= 0 and table[i][0] <= address < table[i][0] + max(table[i][1], 1):
            return table[i][2]
        return f"{lib['name']}+{address:#x}"

    owned_threads = sorted(all_threads(profile), key=lambda pair: -len(pair[0]['samples']['stack']))
    threads = [thread for thread, _ in owned_threads]
    if args.threads:
        for t in threads[:15]:
            print(len(t['samples']['stack']), t['name'])
        return
    thread = next((t for t in threads if not args.thread or t['name'] == args.thread), None)
    if thread is None:
        parser.error(f'no matching thread: {args.thread!r}')
    libs = next(libraries for candidate, libraries in owned_threads if candidate is thread)
    frames, funcs, stacks = thread['frameTable'], thread['funcTable'], thread['stackTable']
    resources = thread['resourceTable']
    names = {}

    def name(frame):
        if frame not in names:
            func = frames['func'][frame]
            resource = funcs['resource'][func]
            lib = resources['lib'][resource] if resource is not None and resource >= 0 else None
            address = frames['address'][frame]
            if lib is None or address is None or address < 0:
                names[frame] = thread['stringArray'][funcs['name'][func]]
            else:
                names[frame] = resolve(lib, address)
        return names[frame]

    self_count, total_count = collections.Counter(), collections.Counter()
    samples = 0
    weights = thread['samples'].get('weight') or [1] * len(thread['samples']['stack'])
    for stack, weight in zip(thread['samples']['stack'], weights):
        chain = []
        while stack is not None:
            chain.append(name(stacks['frame'][stack]))
            stack = stacks['prefix'][stack]
        if not chain or (args.under and not any(args.under in f for f in chain)):
            continue
        samples += weight
        self_count[chain[0]] += weight
        for f in set(chain):
            total_count[f] += weight
    print(f"thread {thread['name']}: {samples} samples")
    for title, counter in (('SELF', self_count), ('INCLUSIVE', total_count)):
        print(f'--- {title}')
        for f, n in counter.most_common(args.top):
            print(f'{100 * n / max(samples, 1):6.2f}%  {f[:150]}')


if __name__ == '__main__':
    main()
