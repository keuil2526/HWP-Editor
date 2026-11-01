"""Read-only structural survey of HWP (CFB) / HWPX (ZIP) files."""
import sys, zipfile, struct, os, collections, zlib

CFB = b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1'


def cfb_streams(d):
    ss = 1 << struct.unpack_from('<H', d, 30)[0]
    nfat = struct.unpack_from('<I', d, 44)[0]
    dirstart = struct.unpack_from('<I', d, 48)[0]
    difat = list(struct.unpack_from('<109I', d, 76))[:min(nfat, 109)]
    fat = []
    for s in difat:
        fat += struct.unpack_from('<%dI' % (ss // 4), d, (s + 1) * ss)
    out = {}
    s, seen = dirstart, set()
    while s < len(fat) and s not in seen:
        seen.add(s)
        off = (s + 1) * ss
        for i in range(ss // 128):
            e = d[off + i * 128: off + i * 128 + 128]
            if len(e) < 128:
                break
            nl = struct.unpack_from('<H', e, 64)[0]
            if 2 < nl <= 64 and e[66] == 2:
                name = e[:nl - 2].decode('utf-16le', 'replace')
                start, size = struct.unpack_from('<II', e, 116)
                out[name] = (start, size)
        s = fat[s]
    return out, ss, fat


def read_stream(d, ss, fat, start, size):
    # only big-sector streams (FileHeader is 256B -> mini stream; handle via root)
    buf, s, seen = b'', start, set()
    while s < len(fat) and s not in seen and len(buf) < size:
        seen.add(s)
        buf += d[(s + 1) * ss:(s + 2) * ss]
        s = fat[s]
    return buf[:size]


agg = collections.Counter()
for p in sys.argv[1:]:
    b = os.path.basename(p)[:38]
    with open(p, 'rb') as f:
        d = f.read()
    if d[:8] == CFB:
        try:
            st, ss, fat = cfb_streams(d)
        except Exception as e:
            print('HWP  parse-err', e, b)
            continue
        names = sorted(st)
        secs = sum(n.startswith('Section') for n in names)
        bins = sum(n.startswith('BIN') for n in names)
        other = [n for n in names if not n.startswith(('Section', 'BIN'))]
        print(f'HWP  {len(d)//1024:6}KB sections={secs:2} bin={bins:3} {",".join(other)[:110]} | {b}')
        for n in other:
            agg['hwp stream: ' + n] += 1
        agg['hwp files'] += 1
    elif d[:2] == b'PK':
        z = zipfile.ZipFile(p)
        il = z.infolist()
        first = il[0]
        mt = z.read('mimetype').decode(errors='replace') if 'mimetype' in z.namelist() else '?'
        bins = sum(i.filename.startswith('BinData/') for i in il)
        print(f'HWPX {len(d)//1024:6}KB entries={len(il):3} bin={bins:3} first={first.filename}'
              f'({"stored" if first.compress_type == 0 else "deflated"}) mime={mt} | {b}')
        for i in il:
            n = i.filename
            if not n.startswith('BinData/'):
                agg['hwpx entry: ' + n] += 1
        agg['hwpx files'] += 1
    else:
        print('UNKNOWN', d[:8], b)

print('\n== aggregate ==')
for k, v in sorted(agg.items()):
    print(f'{v:3} {k}')
