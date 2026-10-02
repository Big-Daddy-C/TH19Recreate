"""Bounded, read-only lookups into TH19's existing decompile and original executable.
Python 3.10+; only `asm` needs the already-installed optional capstone package.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / 'research/catalog.json'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources(root):
    result = {}
    for path in sorted(root.rglob('*.c')):
        match = re.match(r'([0-9a-fA-F]{8})_', path.name)
        if match:
            address = match[1].lower()
            if address in result:
                raise ValueError(f'Duplicate function address {address}: {path}')
            result[address] = path
    return result


def resolve(value, catalog):
    if value in catalog['symbols']:
        return catalog['symbols'][value]['address']
    value = value.lower().removeprefix('fun_').removeprefix('0x')
    if not re.fullmatch(r'[0-9a-f]{1,8}', value):
        raise ValueError(f'Unknown function alias/address: {value}')
    return value.zfill(8)


def bounded(lines, start, limit):
    if start < 1 or limit < 1:
        raise ValueError('Start and limit must be positive')
    selected = lines[start-1:start-1+limit]
    result = [f'{i}: {line}' for i, line in enumerate(selected, start)]
    end = start - 1 + len(selected)
    if end < len(lines):
        result.append(f'[truncated: {len(lines)-end} more lines; resume with --start {end+1}]')
    return result


def pe_bytes(data, va, size):
    if data[:2] != b'MZ':
        raise ValueError('Expected MZ executable')
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    if data[pe:pe+4] != b'PE\0\0':
        raise ValueError('Expected PE signature')
    machine, nsec = struct.unpack_from('<HH', data, pe+4)
    opt = struct.unpack_from('<H', data, pe+20)[0]
    if machine != 0x14c or struct.unpack_from('<H', data, pe+24)[0] != 0x10b:
        raise ValueError('Only original x86 PE32 is supported')
    base = struct.unpack_from('<I', data, pe+52)[0]
    rva = va-base
    for i in range(nsec):
        offset = pe+24+opt+i*40
        _, section_va, raw_size, raw = struct.unpack_from('<IIII', data, offset+8)
        if section_va <= rva < section_va+raw_size:
            count = min(size, section_va+raw_size-rva)
            offset = raw+rva-section_va
            if offset+count > len(data):
                raise ValueError('Truncated section')
            return data[offset:offset+count]
    raise ValueError(f'Address {va:08x} has no file-backed section')


def verify(catalog, paths, exe):
    errors = []
    if not exe.is_file() or digest(exe) != catalog['reference']['exe_sha256']:
        errors.append('Executable absent or SHA256 differs from recorded v1.10c reference')
    for alias, symbol in catalog['symbols'].items():
        path = paths.get(symbol['address'])
        expected = symbol.get('decompile_sha256')
        if expected and (path is None or digest(path) != expected):
            errors.append(f'{alias}: decompile absent or changed; recheck its findings')
        elif expected is None and path is not None:
            errors.append(f'{alias}: newly exported function; update the recorded source status')
    for name, feature in catalog['features'].items():
        if feature['implementation'] not in ('missing', 'partial', 'implemented', 'disabled'):
            errors.append(f'{name}: invalid implementation status')
        for alias in feature['symbols']:
            if alias not in catalog['symbols']:
                errors.append(f'{name}: missing symbol {alias}')
        for path in feature['code'] + feature['tests']:
            if not (ROOT / path).is_file():
                errors.append(f'{name}: missing repository file {path}')
        for fact in feature['findings']:
            if fact['evidence'] not in ('code_checked', 'prior_session', 'user_observation', 'assumption'):
                errors.append(f'{name}: invalid evidence status')
            for alias in fact['sources']:
                if alias not in catalog['symbols']:
                    errors.append(f'{name}: invalid finding source {alias}')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path(os.environ.get('TH19_DECOMPILE', str(Path.home()/'ghidra-export-normalized-v4/functions'))))
    parser.add_argument('--exe', type=Path, default=Path(os.environ.get('TH19_EXE', str(Path.home()/'Downloads/th19/th19/th19.exe'))))
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('check', help='Validate evidence fingerprints and catalog links; does not prove gameplay parity')
    topic = sub.add_parser('topic', help='Compact feature brief, findings and next work')
    topic.add_argument('name', nargs='?')
    show = sub.add_parser('show', help='Read several functions in one call, by alias or address')
    show.add_argument('names', nargs='+')
    show.add_argument('--start', type=int, default=1)
    show.add_argument('--limit', type=int, default=80, help='Lines per function')
    search = sub.add_parser('search', help='Literal source search, with bounded output')
    search.add_argument('text')
    search.add_argument('--limit', type=int, default=20)
    callers = sub.add_parser('callers', help='Textual FUN references only; indirect calls are not covered')
    callers.add_argument('name')
    callers.add_argument('--limit', type=int, default=20)
    for name in ('asm', 'table'):
        p = sub.add_parser(name, help='Inspect executable bytes using a checked reference fingerprint')
        p.add_argument('address')
        p.add_argument('--count', type=int, default=24 if name == 'asm' else 8)
    args = parser.parse_args()
    catalog = json.loads(CATALOG.read_text(encoding='utf-8'))
    if args.command == 'topic':
        if not args.name:
            for name, f in catalog['features'].items():
                print(f"{name}: {f['implementation']} - {f['summary']}")
            return 0
        f = catalog['features'].get(args.name)
        if f is None:
            raise ValueError('Unknown topic; run topic without a name')
        print(json.dumps(f, indent=2, ensure_ascii=False))
        print('SYMBOLS:')
        for alias in f['symbols']:
            sym = catalog['symbols'][alias]
            print(f"  {alias} = {sym['address']} ({'exported' if sym['decompile_sha256'] else 'binary only'})")
        return 0
    if args.command in ('asm', 'table'):
        if not 1 <= args.count <= 1000:
            raise ValueError('Count must be 1..1000')
        if digest(args.exe) != catalog['reference']['exe_sha256']:
            raise ValueError('Executable fingerprint mismatch; addresses may refer to another build')
        va = int(resolve(args.address, catalog), 16)
        data = args.exe.read_bytes()
        if args.command == 'table':
            blob = pe_bytes(data, va, args.count*4)
            if len(blob) != args.count*4:
                raise ValueError('Table crosses file-backed section boundary')
            for i, (v,) in enumerate(struct.iter_unpack('<I', blob)):
                print(f'{va+i*4:08x}: {v:08x}')
        else:
            try:
                import capstone
            except ImportError:
                raise ValueError('asm requires capstone; all other commands use only Python standard library')
            decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
            instructions = list(decoder.disasm(pe_bytes(data, va, args.count*15), va, count=args.count))
            if not instructions:
                raise ValueError('No instructions decoded at the supplied address')
            for ins in instructions:
                print(f'{ins.address:08x}: {ins.mnemonic} {ins.op_str}')
            print('Linear disassembly from supplied address; not a recovered function boundary.')
        return 0
    if not args.source.is_dir():
        raise ValueError('Decompile directory missing; set TH19_DECOMPILE or --source')
    paths = sources(args.source)
    if args.command == 'check':
        errors = verify(catalog, paths, args.exe)
        print(f'{len(paths)} exported function files; {len(catalog["symbols"])} named references; {len(catalog["features"])} topics')
        for error in errors:
            print('ERROR:', error)
        print('Catalog integrity: ' + ('FAIL' if errors else 'PASS') + '; original-game parity is not assessed.')
        return int(bool(errors))
    if args.limit < 1 or args.limit > 2000:
        raise ValueError('Limit must be 1..2000')
    if args.command == 'show':
        missing = False
        for name in args.names:
            address = resolve(name, catalog)
            path = paths.get(address)
            if path is None:
                print(f'{name} [{address}]: NO EXPORTED FUNCTION; use asm/table or repair Ghidra analysis')
                missing = True
                continue
            print(f'{name} [{address}] {path}')
            print('\n'.join(bounded(path.read_text(encoding='utf-8').splitlines(), args.start, args.limit)))
        return int(missing)
    needle = args.text if args.command == 'search' else 'FUN_' + resolve(args.name, catalog)
    total = 0
    for address, path in paths.items():
        for i, line in enumerate(path.read_text(encoding='utf-8').splitlines(), 1):
            if needle.lower() in line.lower():
                if args.command == 'callers' and address == resolve(args.name, catalog):
                    continue
                if total < args.limit:
                    print(f'{path}:{i}: {line.strip()}')
                total += 1
    print(f'{total} matching lines; {max(0,total-args.limit)} omitted. Literal text matches are not proof of a call graph.')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, KeyError, struct.error) as exc:
        print(f'Error: {exc}', file=sys.stderr)
        sys.exit(1)
