"""Independent (non-rhwp) structural diff for HWP 5.0 (CFB) and HWPX (ZIP) files.

Usage: python3 -I hwpdiff.py ORIG RESULT [--json]
Compares container entries byte-for-byte, then HWP record streams record-by-record
and HWPX XML parts element-by-element. Stdlib only.
"""
import sys, struct, zlib, zipfile, hashlib, json, difflib
import xml.etree.ElementTree as ET

CFB_MAGIC = b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1'
END = 0xFFFFFFFE
FREE = 0xFFFFFFFF


def sha(b):
    return hashlib.sha256(b).hexdigest()[:16]


# ---------------------------------------------------------------- CFB reader
class CFB:
    def __init__(self, d):
        if d[:8] != CFB_MAGIC:
            raise ValueError('not a CFB file')
        self.d = d
        self.ss = 1 << struct.unpack_from('<H', d, 30)[0]
        self.mss = 1 << struct.unpack_from('<H', d, 32)[0]
        nfat, dirstart = struct.unpack_from('<II', d, 44)
        self.cutoff, minifat_start, nminifat, difat_start, ndifat = struct.unpack_from('<IIIII', d, 56)
        difat = list(struct.unpack_from('<109I', d, 76))
        s = difat_start
        per = self.ss // 4
        for _ in range(ndifat):
            if s >= END:
                break
            ent = struct.unpack_from('<%dI' % per, d, self.sec_off(s))
            difat += ent[:-1]
            s = ent[-1]
        self.fat = []
        for s in difat[:nfat]:
            self.fat += struct.unpack_from('<%dI' % per, d, self.sec_off(s))
        dirdata = self.chain(dirstart)
        self.entries = []
        for i in range(len(dirdata) // 128):
            e = dirdata[i * 128:(i + 1) * 128]
            nl = struct.unpack_from('<H', e, 64)[0]
            typ = e[66]
            name = e[:max(nl - 2, 0)].decode('utf-16le', 'replace')
            left, right, child = struct.unpack_from('<III', e, 68)
            start = struct.unpack_from('<I', e, 116)[0]
            size = struct.unpack_from('<Q', e, 120)[0] & 0xFFFFFFFF
            self.entries.append(dict(name=name, type=typ, left=left, right=right,
                                     child=child, start=start, size=size))
        root = self.entries[0]
        self.ministream = self.chain(root['start'])[:root['size']] if root['start'] < END else b''
        mf = self.chain(minifat_start) if minifat_start < END else b''
        self.minifat = list(struct.unpack_from('<%dI' % (len(mf) // 4), mf))

    def sec_off(self, s):
        return (s + 1) * self.ss

    def chain(self, s, fat=None, ss=None, src=None):
        fat = fat or self.fat
        ss = ss or self.ss
        out, seen = [], set()
        while s < END and s not in seen and s < len(fat):
            seen.add(s)
            if src is None:
                out.append(self.d[self.sec_off(s):self.sec_off(s) + ss])
            else:
                out.append(src[s * ss:(s + 1) * ss])
            s = fat[s]
        return b''.join(out)

    def read(self, e):
        if e['size'] < self.cutoff:
            return self.chain(e['start'], self.minifat, self.mss, self.ministream)[:e['size']]
        return self.chain(e['start'])[:e['size']]

    def streams(self):
        """Return {path: bytes} walking the red-black tree from root."""
        out = {}

        def walk(idx, prefix, depth=0):
            if idx >= len(self.entries) or depth > 64:
                return
            stack, seen = [idx], set()
            while stack:
                i = stack.pop()
                if i >= len(self.entries) or i in seen:
                    continue
                seen.add(i)
                e = self.entries[i]
                stack += [e['left'], e['right']]
                p = prefix + e['name']
                if e['type'] == 2:
                    out[p] = self.read(e)
                elif e['type'] == 1:
                    walk(e['child'], p + '/', depth + 1)
        walk(self.entries[0]['child'], '')
        return out


# ---------------------------------------------------------------- HWP records
HWPTAG_BEGIN = 0x10
TAG_NAMES = {
    16: 'DOCUMENT_PROPERTIES', 17: 'ID_MAPPINGS', 18: 'BIN_DATA', 19: 'FACE_NAME', 20: 'BORDER_FILL',
    21: 'CHAR_SHAPE', 22: 'TAB_DEF', 23: 'NUMBERING', 24: 'BULLET', 25: 'PARA_SHAPE', 26: 'STYLE',
    27: 'DOC_DATA', 28: 'DISTRIBUTE_DOC_DATA', 30: 'COMPATIBLE_DOCUMENT', 31: 'LAYOUT_COMPATIBILITY',
    32: 'TRACKCHANGE', 76: 'MEMO_SHAPE', 78: 'FORBIDDEN_CHAR', 80: 'TRACK_CHANGE', 81: 'TRACK_CHANGE_AUTHOR',
    66: 'PARA_HEADER', 67: 'PARA_TEXT', 68: 'PARA_CHAR_SHAPE', 69: 'PARA_LINE_SEG', 70: 'PARA_RANGE_TAG',
    71: 'CTRL_HEADER', 72: 'LIST_HEADER', 73: 'PAGE_DEF', 74: 'FOOTNOTE_SHAPE', 75: 'PAGE_BORDER_FILL',
    76 + 0x100: '', 77: 'TABLE', 78 + 0x100: '', 79: 'SHAPE_COMPONENT', 87: 'CTRL_DATA', 88: 'EQEDIT',
    85: 'SHAPE_COMPONENT_PICTURE', 86: 'SHAPE_COMPONENT_OLE', 89: 'SHAPE_COMPONENT_RECT',
}


def tagname(t):
    return TAG_NAMES.get(t, 'TAG_%d' % t)


def parse_records(b):
    recs, i = [], 0
    while i + 4 <= len(b):
        h = struct.unpack_from('<I', b, i)[0]
        tag, lvl, size = h & 0x3FF, (h >> 10) & 0x3FF, h >> 20
        i += 4
        if size == 0xFFF:
            size = struct.unpack_from('<I', b, i)[0]
            i += 4
        recs.append((tag, lvl, b[i:i + size]))
        i += size
    return recs, i == len(b)


def hwp_compressed(streams):
    fh = streams.get('FileHeader', b'')
    if len(fh) < 40 or not fh.startswith(b'HWP Document File'):
        return None
    return bool(struct.unpack_from('<I', fh, 36)[0] & 1)


def inflate(b):
    try:
        return zlib.decompress(b, -15), True
    except zlib.error:
        return b, False


def record_diff(ra, rb, limit=12):
    ka = ['%s/L%d/%s' % (tagname(t), l, sha(p)) for t, l, p in ra]
    kb = ['%s/L%d/%s' % (tagname(t), l, sha(p)) for t, l, p in rb]
    sm = difflib.SequenceMatcher(a=ka, b=kb, autojunk=False)
    ops, changed = [], 0
    for op, a1, a2, b1, b2 in sm.get_opcodes():
        if op == 'equal':
            continue
        changed += max(a2 - a1, b2 - b1)
        if len(ops) < limit:
            ops.append(dict(op=op, orig=[k.rsplit('/', 1)[0] for k in ka[a1:a2]][:6],
                            result=[k.rsplit('/', 1)[0] for k in kb[b1:b2]][:6], at=a1))
    return dict(orig_records=len(ra), result_records=len(rb), changed_records=changed, ops=ops)


def diff_hwp(da, db):
    A, B = CFB(da).streams(), CFB(db).streams()
    ca, cb = hwp_compressed(A), hwp_compressed(B)
    rep = dict(format='hwp', compressed=[ca, cb], added=sorted(set(B) - set(A)),
               removed=sorted(set(A) - set(B)), identical=[], changed={})
    for n in sorted(set(A) & set(B)):
        if A[n] == B[n]:
            rep['identical'].append(n)
            continue
        info = dict(orig_size=len(A[n]), result_size=len(B[n]))
        is_rec = n == 'DocInfo' or n.startswith('BodyText/') or n.startswith('ViewText/')
        pa, pb = A[n], B[n]
        if ca and (is_rec or n.startswith('BinData/')):
            pa, oka = inflate(pa)
            pb, okb = inflate(pb)
            info['inflate_ok'] = [oka, okb]
        if pa == pb:
            info['payload_identical'] = True  # only compression bytes differ
        elif is_rec:
            ra, fa = parse_records(pa)
            rb, fb = parse_records(pb)
            info['record_parse_ok'] = [fa, fb]
            info.update(record_diff(ra, rb))
        rep['changed'][n] = info
    return rep


# ---------------------------------------------------------------- HWPX
def xml_items(b):
    """Flatten an XML document to a list of 'path attrs text' strings."""
    try:
        root = ET.fromstring(b)
    except ET.ParseError as e:
        return None, str(e)
    out = []

    def walk(el, path):
        tag = el.tag.split('}')[-1]
        p = path + '/' + tag
        attrs = ' '.join('%s=%s' % (k.split('}')[-1], v) for k, v in sorted(el.attrib.items()))
        out.append('%s [%s] %r' % (p, attrs, (el.text or '').strip()[:80]))
        for c in el:
            walk(c, p)
    walk(root, '')
    return out, None


def diff_hwpx(pa, pb):
    za, zb = zipfile.ZipFile(pa), zipfile.ZipFile(pb)
    ia = {i.filename: i for i in za.infolist()}
    ib = {i.filename: i for i in zb.infolist()}
    order_a = [i.filename for i in za.infolist()]
    order_b = [i.filename for i in zb.infolist()]
    first = zb.infolist()[0]
    rep = dict(format='hwpx', added=sorted(set(ib) - set(ia)), removed=sorted(set(ia) - set(ib)),
               order_same=[n for n in order_a if n in ib] == [n for n in order_b if n in ia],
               mimetype_first_stored=(first.filename == 'mimetype' and first.compress_type == 0),
               identical=[], changed={})
    for n in order_a:
        if n not in ib:
            continue
        a, b = za.read(n), zb.read(n)
        if a == b:
            rep['identical'].append(n)
            if ia[n].compress_type != ib[n].compress_type:
                rep['changed'][n] = dict(only_compression=[ia[n].compress_type, ib[n].compress_type])
            continue
        info = dict(orig_size=len(a), result_size=len(b))
        if n.endswith(('.xml', '.hpf', '.rdf')):
            xa, ea = xml_items(a)
            xb, eb = xml_items(b)
            if xa is None or xb is None:
                info['xml_error'] = [ea, eb]
            else:
                sm = difflib.SequenceMatcher(a=xa, b=xb, autojunk=False)
                ops, changed = [], 0
                for op, a1, a2, b1, b2 in sm.get_opcodes():
                    if op == 'equal':
                        continue
                    changed += max(a2 - a1, b2 - b1)
                    if len(ops) < 8:
                        ops.append(dict(op=op, orig=xa[a1:a2][:3], result=xb[b1:b2][:3]))
                info.update(orig_nodes=len(xa), result_nodes=len(xb), changed_nodes=changed, ops=ops)
        rep['changed'][n] = info
    return rep


def diff(pa, pb):
    da, db = open(pa, 'rb').read(), open(pb, 'rb').read()
    base = dict(orig=pa, result=pb, orig_sha=sha(da), result_sha=sha(db), byte_identical=da == db)
    if da == db:
        return base
    if da[:8] == CFB_MAGIC and db[:8] == CFB_MAGIC:
        base.update(diff_hwp(da, db))
    elif da[:2] == b'PK' and db[:2] == b'PK':
        base.update(diff_hwpx(pa, pb))
    else:
        base['format_changed'] = [da[:8].hex(), db[:8].hex()]
    return base


if __name__ == '__main__':
    r = diff(sys.argv[1], sys.argv[2])
    if '--json' in sys.argv:
        print(json.dumps(r, ensure_ascii=False, indent=1))
    else:
        print('byte_identical:', r['byte_identical'])
        for k in ('added', 'removed', 'format_changed', 'order_same', 'mimetype_first_stored', 'compressed'):
            if k in r:
                print(k + ':', r[k])
        if 'identical' in r:
            print('identical entries:', len(r['identical']))
        for n, info in r.get('changed', {}).items():
            print('CHANGED', n, {k: v for k, v in info.items() if k != 'ops'})
            for op in info.get('ops', [])[:4]:
                print('   ', op)
