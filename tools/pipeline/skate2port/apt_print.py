"""Prints a runtime manifest's ActionScript as rough pseudo-code, and lists opcodes the engine VM can't run.

usage: py apt_print.py MANIFEST.json [FILTER]   (FILTER: only functions whose name/assignment contains it)
"""
import json, sys

SUPPORTED = {0, 0x70, 0xa1, 0xa4, 0xa5, 0x26, 6, 7, 0x81, 0x8c, 0x17, 0x12, 0x4c, 0x59, 0x5a, 0x71, 0x73, 0x74, 0x75,
             0x76, 0xb5, 0xb6, 0xb7, 0xb4, 0xb9, 0x87, 0x88, 0x96, 0xa2, 0xa3, 0xae, 0x4e, 0xaf, 0x4f, 0x1c, 0x1d, 0x3c,
             0x3a, 0x3b, 0x47, 0x0b, 0x0c, 0x0d, 0x48, 0x49, 0x66, 0x67, 0x42, 0x43, 0x3f, 0x4a, 0x30, 0x50, 0x51, 0x99,
             0x9d, 0xb8, 0x8e, 0x9b, 0x69, 0x40, 0x52, 0xb0, 0xb1, 0xb2, 0xb3, 0x3d, 0x5d, 0x3e}
BIN = {0x47: '+', 0x0b: '-', 0x0c: '*', 0x0d: '/', 0x48: '<', 0x67: '>', 0x66: '===', 0x49: '==', 0x3f: '%'}


def const(c):
    return repr(c['value']) if c['kind'] == 1 else ('undefined' if c['kind'] == 3 else str(c['value']))


def block(code, out, depth, unsupported, consts=None):
    consts = list(consts or [])
    st = []
    pad = '  ' * depth
    pop = lambda: st.pop() if st else '?'
    for i in code:
        op, operand = i['opcode'], i.get('operand')
        if op not in SUPPORTED:
            unsupported.add(op)
        if op == 0x88:
            consts = [const(c) for c in i.get('values', [])]
        elif op == 0x96:
            st += [f"r{c['value']}" if c['kind'] == 4 else const(c) for c in i.get('values', [])]
        elif op in (0xa2, 0xa3):
            st.append(consts[operand] if operand < len(consts) else f'c{operand}')
        elif op == 0xa1:
            st.append(repr(operand))
        elif op in (0xa4, 0xae):
            st.append(operand if op == 0xa4 else (consts[operand].strip("'") if operand < len(consts) else f'c{operand}'))
        elif op == 0xa5:
            st.append(f'{pop()}.{operand}')
        elif op in (0x4e, 0xaf):
            name = (consts[operand] if op == 0xaf else pop()).strip("'")
            st.append(f'{pop()}.{name}')
        elif op == 0x1c:
            st.append(pop().strip("'"))
        elif op == 0x70:
            st.append('this')
        elif op == 0x71:
            st.append('_global')
        elif op in (0x59, 0x5a):
            st.append('0' if op == 0x59 else '1')
        elif op in (0x73, 0x74, 0x75, 0x76):
            st.append({0x73: 'true', 0x74: 'false'}.get(op, 'undefined'))
        elif op in (0xb5, 0xb6, 0xb7, 0xb4):
            st.append(str(operand))
        elif op == 0xb9:
            st.append(f'r{operand}')
        elif op == 0x87:
            out.append(f'{pad}r{operand} = {st[-1] if st else "?"}')
        elif op == 0x4c:
            st.append(st[-1] if st else '?')
        elif op == 0x4f:
            v, k, o = pop(), pop().strip("'"), pop()
            out.append(f'{pad}{o}.{k} = {v}')
        elif op in (0x1d, 0x3c):
            v, k = pop(), pop().strip("'")
            out.append(f'{pad}{"var " if op == 0x3c else ""}{k} = {v}')
        elif op in BIN:
            b, a = pop(), pop()
            st.append(f'({a} {BIN[op]} {b})')
        elif op == 0x12:
            st.append(f'!{pop()}')
        elif op in (0x50, 0x51):
            st.append(f'({pop()} {"+" if op == 0x50 else "-"} 1)')
        elif op in (0x42, 0x43):
            n = pop()
            try:
                n = int(float(n))
            except ValueError:
                n = 0
            items = [pop() for _ in range(n * (1 if op == 0x42 else 2))]
            st.append(f'[{", ".join(items)}]' if op == 0x42 else '{...}')
        elif op == 0x40:
            name, n = pop().strip("'"), pop()
            try:
                n = int(float(n))
            except ValueError:
                n = 0
            st.append(f'new {name}({", ".join(pop() for _ in range(n))})')
        elif op in (0x52, 0xb0, 0xb1, 0xb2, 0xb3, 0x3d, 0x5d):
            method = (consts[operand] if op in (0xb0, 0xb1, 0xb2, 0xb3) else pop()).strip("'")
            obj = None if op in (0x3d, 0xb0, 0xb1) else pop()
            n = pop()
            try:
                n = int(float(n))
            except ValueError:
                n = 0
            args = ', '.join(pop() for _ in range(n))
            call = f'{obj + "." if obj else ""}{method}({args})'
            if op in (0xb1, 0xb3):
                out.append(f'{pad}return {call}')
            elif op in (0xb0, 0xb2, 0x5d):
                out.append(f'{pad}{call}')
            else:
                st.append(call)
        elif op == 0x17:
            if st:
                v = st.pop()
                if '(' in v:
                    out.append(f'{pad}{v}')
        elif op == 0x3e:
            out.append(f'{pad}return {pop()}')
        elif op in (0x99, 0x9d, 0xb8):
            cond = '' if op == 0x99 else f'if ({"!" if op == 0xb8 else ""}{pop()}) '
            out.append(f'{pad}{cond}goto {i.get("target"):x}' if i.get('target') is not None else f'{pad}{cond}goto ?')
        elif op in (0x8e, 0x9b):
            params = ', '.join(p.get('name', '?') for p in i.get('parameters', []))
            name = i.get('name') or ''
            sub = []
            block(i.get('body', []), sub, depth + 1, unsupported, consts)
            text = f'function {name}({params}) {{\n' + '\n'.join(sub) + f'\n{pad}}}'
            if name:
                out.append(pad + text)
            else:
                st.append(text)
        elif op == 0x69:
            b, a = pop(), pop()
            out.append(f'{pad}{a} extends {b}')
        elif op in (6, 7, 0x81, 0x8c, 0x26):
            out.append(f'{pad}{["play()", "stop()"][op - 6] if op in (6, 7) else f"gotoAndStop({operand})" if op != 0x26 else f"trace({pop()})"}')
        elif op == 0:
            break
        # labels for jumps
        if any(True for _ in ()):
            pass
    return consts


if __name__ == '__main__':
    m = json.load(open(sys.argv[1], encoding='utf-8'))
    filt = sys.argv[2] if len(sys.argv) > 2 else None
    unsupported = set()
    for key, code in m['actions'].items():
        out = []
        block(code, out, 0, unsupported)
        text = '\n'.join(out)
        if filt is None:
            print(f'// block {key}\n{text}\n')
        else:
            for line_block in text.split('\n\n'):
                if filt in line_block:
                    print(f'// block {key}\n{line_block}\n')
    print('UNSUPPORTED OPCODES:', sorted(hex(o) for o in unsupported) or 'none', file=sys.stderr)
