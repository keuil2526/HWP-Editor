"""Check documents against Hancom Office (Hwp 2022) through the WinBoat bridge.

The Windows side runs ~/hwpedit-bridge/hancom-worker.ps1, which opens each job in
Hwp via its automation object and returns page count, extracted text, a PDF and a
Hwp re-save. This side compares those with hwpedit's own engine and renderer.

Usage:
  python3 -I tools/hancom.py check OUT_DIR FILE...   # submit, wait, compare, report
  python3 -I tools/hancom.py ping                    # is the worker running?
  python3 -I tools/hancom.py tour OUT_DIR            # screenshots of Hwp's menus, ribbon, dialogs
  python3 -I tools/hancom.py ui OUT_DIR STEPS.json    # run UI steps (click, shot, …) and save the shots

Environment: HWPEDIT_BRIDGE (default ~/hwpedit-bridge).
Needs: target/release/hwpedit-verify, target/release/examples/render_pages,
pdftoppm (poppler), Pillow.
"""
import difflib, hashlib, json, os, re, shutil, subprocess, sys, time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
VERIFY = REPO / 'target/release/hwpedit-verify'
RENDER = REPO / 'target/release/examples/render_pages'
BRIDGE = Path(os.environ.get('HWPEDIT_BRIDGE', Path.home() / 'hwpedit-bridge'))
INBOX, OUTBOX = BRIDGE / 'inbox', BRIDGE / 'outbox'


def submit(path, ops=('text', 'pdf', 'resave')):
    path = Path(path)
    data = path.read_bytes()
    fmt = 'hwpx' if data[:2] == b'PK' else 'hwp'  # by content, like the editor
    jid = '%s-%d' % (hashlib.sha256(data).hexdigest()[:10], time.time_ns() % 10**9)
    INBOX.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(path, INBOX / f'{jid}.{fmt}')
    tmp = INBOX / f'{jid}.job.tmp'
    tmp.write_text(json.dumps({'file': f'{jid}.{fmt}', 'ops': list(ops)}), encoding='utf-8')
    tmp.rename(INBOX / f'{jid}.job.json')  # the worker only picks up complete jobs
    return jid, fmt


def wait(jid, timeout):
    res = OUTBOX / f'{jid}.result.json'
    end = time.time() + timeout
    while time.time() < end:
        if res.exists():
            time.sleep(0.3)  # let the share flush
            return json.loads(res.read_text(encoding='utf-8-sig'))
        time.sleep(0.5)
    return None


def wait_all(jids, stall):
    """Wait for every job; give up only when no job finished for `stall` seconds
    (the worker runs jobs one at a time, in name order)."""
    done, last = {}, time.time()
    while len(done) < len(jids) and time.time() - last < stall:
        for j in jids:
            if j not in done and (OUTBOX / f'{j}.result.json').exists():
                time.sleep(0.3)
                done[j] = json.loads((OUTBOX / f'{j}.result.json').read_text(encoding='utf-8-sig'))
                last = time.time()
                print(f'  {len(done)}/{len(jids)}', file=sys.stderr, flush=True)
        time.sleep(0.5)
    return done


def norm(text):
    return re.sub(r'\s+', '', text)


def text_similarity(a, b):
    a, b = norm(a), norm(b)
    if not a and not b:
        return 1.0
    return difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()


def ink_mask(img, size):
    from PIL import Image
    g = img.convert('L').crop((0, 0) + size)
    return g.point(lambda v: 255 if v < 200 else 0)


def page_match(ours_path, hancom_png):
    """F1 of ink pixels with 2 px tolerance (silhouette match)."""
    from PIL import Image, ImageChops, ImageFilter
    a, b = Image.open(ours_path), Image.open(hancom_png)
    size = (min(a.width, b.width), min(a.height, b.height))
    ma, mb = ink_mask(a, size), ink_mask(b, size)
    da, db = ma.filter(ImageFilter.MaxFilter(5)), mb.filter(ImageFilter.MaxFilter(5))

    def count(m):
        return m.histogram()[255]

    na, nb = count(ma), count(mb)
    if na == 0 and nb == 0:
        return 1.0, (a.size, b.size)
    precision = count(ImageChops.multiply(ma, db)) / na if na else 0.0
    recall = count(ImageChops.multiply(mb, da)) / nb if nb else 0.0
    f1 = 0.0 if precision + recall == 0 else 2 * precision * recall / (precision + recall)
    return f1, (a.size, b.size)


def ours(path):
    out = subprocess.run([str(VERIFY), '--info', str(path)], capture_output=True, text=True).stdout
    return json.loads(out.strip().splitlines()[-1])


def check(out_dir, files, timeout=600):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    jobs = [(f, *submit(f)) for f in files]
    results = wait_all([j for _, j, _ in jobs], timeout)
    rows = []
    for f, jid, fmt in jobs:
        r = {'file': str(f), 'job': jid}
        res = results.get(jid)
        if res is None:
            r['hancom'] = 'timeout (작업기가 실행 중인지 확인하세요)'
            rows.append(r)
            continue
        r['hancom_opened'] = res.get('opened')
        r['hancom_pages'] = res.get('pages')
        r['hancom_error'] = res.get('error')
        mine = ours(f)
        r['our_pages'] = mine.get('pages')
        r['our_error'] = mine.get('error')
        d = out_dir / jid
        d.mkdir(exist_ok=True)
        for name in (f'{jid}.txt', f'{jid}.pdf', f'{jid}.resaved.{fmt}'):
            if (OUTBOX / name).exists():
                shutil.move(str(OUTBOX / name), d / name)
        (OUTBOX / f'{jid}.result.json').unlink(missing_ok=True)
        if (d / f'{jid}.txt').exists() and 'text' in mine:
            r['text_similarity'] = round(text_similarity(mine['text'], (d / f'{jid}.txt').read_text(encoding='utf-8-sig')), 4)
        pdf = d / f'{jid}.pdf'
        if pdf.exists() and not mine.get('error'):
            subprocess.run(['pdftoppm', '-r', '96', '-png', str(pdf), str(d / 'hancom')], check=True)
            subprocess.run([str(RENDER), str(f), str(d / 'ours')], capture_output=True, check=True)
            hancom_pages = sorted(d.glob('hancom-*.png'))
            our_pages = sorted((d / 'ours').glob('page-*.ppm'))
            r['page_match'] = [round(page_match(o, h)[0], 4) for o, h in zip(our_pages, hancom_pages)]
            r['hancom_pdf_pages'] = len(hancom_pages)
        resaved = d / f'{jid}.resaved.{fmt}'
        if resaved.exists():
            back = ours(resaved)
            r['hancom_resave_reopens_in_hwpedit'] = 'error' not in back
        rows.append(r)
    (out_dir / 'report.json').write_text(json.dumps(rows, ensure_ascii=False, indent=1), encoding='utf-8')
    for r in rows:
        pm = r.get('page_match') or []
        print(f"{Path(r['file']).name[:40]:40} opened={r.get('hancom_opened')} "
              f"pages ours/hancom={r.get('our_pages')}/{r.get('hancom_pages')} "
              f"text={r.get('text_similarity')} match_min={min(pm) if pm else None} "
              f"resave_ok={r.get('hancom_resave_reopens_in_hwpedit')} {r.get('hancom_error') or r.get('hancom') or ''}")
    return rows


# Hwp 2022 menu tabs and their Alt access keys.
MENU_KEYS = [('file', 'f'), ('edit', 'e'), ('view', 'u'), ('insert', 'd'), ('format', 'j'),
             ('page', 'w'), ('security', 'r'), ('review', 'h'), ('tools', 'k')]


def tour(out_dir, timeout=600):
    steps = [{'new': True}, {'text': '한글 2022 화면 구성 확인용 문서입니다. 가나다라마바사 ABC 123'}, {'sleep': 600}, {'shot': 'doc'}]
    for name, key in MENU_KEYS:
        steps += [{'keys': '%' + key}, {'sleep': 900}, {'shot': 'menu-' + name}, {'keys': '{ESC}{ESC}'}, {'sleep': 300}]
    for action in ('CharShape', 'ParagraphShape', 'TableCreate'):
        steps += [{'dialog': action, 'shot': 'dialog-' + action}, {'sleep': 500}]
    return ui(out_dir, steps, timeout)


def ui(out_dir, steps, timeout=600):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    INBOX.mkdir(parents=True, exist_ok=True)
    jid = 'ui-%d' % (time.time_ns() % 10**9)
    tmp = INBOX / f'{jid}.job.tmp'
    tmp.write_text(json.dumps({'op': 'ui', 'steps': steps}, ensure_ascii=False), encoding='utf-8')
    tmp.rename(INBOX / f'{jid}.job.json')
    res = wait(jid, timeout)
    if res is None:
        log = OUTBOX / f'{jid}.progress.txt'
        print('작업기 응답 없음. 마지막 진행:', log.read_text(encoding='utf-8-sig').splitlines()[-1:] if log.exists() else '(없음)')
        return None
    for shot in res.get('shots') or []:
        src = OUTBOX / f'{jid}.{shot}.png'
        if src.exists():
            shutil.move(str(src), out_dir / f'{shot}.png')
    (OUTBOX / f'{jid}.result.json').unlink(missing_ok=True)
    print(json.dumps(res, ensure_ascii=False))
    return res


def ping(timeout=240):
    blank = REPO / 'vendor/rhwp/saved/blank2010.hwp'
    jid, _ = submit(blank, ops=())
    res = wait(jid, timeout)
    (OUTBOX / f'{jid}.result.json').unlink(missing_ok=True)
    print('worker: ' + ('응답 없음' if res is None else f"응답함 (opened={res.get('opened')}, {res.get('open_ms')} ms)"))
    status = OUTBOX / 'worker.status'  # written by the worker's watchdog every 5 s
    if res is None and status.exists():
        print('worker.status:', status.read_text(encoding='utf-8-sig').strip())
    return res is not None


if __name__ == '__main__':
    cmd = sys.argv[1] if len(sys.argv) > 1 else ''
    if cmd == 'ping':
        sys.exit(0 if ping() else 1)
    elif cmd == 'tour' and len(sys.argv) > 2:
        sys.exit(0 if tour(sys.argv[2]) else 1)
    elif cmd == 'ui' and len(sys.argv) > 3:
        steps = json.loads(Path(sys.argv[3]).read_text(encoding='utf-8'))
        sys.exit(0 if ui(sys.argv[2], steps) else 1)
    elif cmd == 'check' and len(sys.argv) > 3:
        check(sys.argv[2], sys.argv[3:])
    else:
        print(__doc__)
        sys.exit(2)
