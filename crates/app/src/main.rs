mod glass;
mod icons;
mod render;
mod ribbon;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};
use std::path::Path;
use std::rc::Rc;
use window::Editor;

const APP_ID: &str = "io.github.hwpedit.HwpEdit";

pub const SUPPORT_TEXT: &str = "\
열기: HWP 5.x(.hwp), HWPX(.hwpx). 확장자가 아니라 파일 내용으로 형식을 판별합니다.
편집: 본문 문단과 (중첩되지 않은) 표 셀의 텍스트 입력·삭제·선택(끌기, Shift+방향키, Ctrl+A), 문단 나누기/합치기, 한글 입력기, 실행 취소/다시 실행, 복사·잘라내기·붙여넣기(문서 안에서는 서식 유지), 찾기·바꾸기.
서식: 글꼴·크기·진하게·기울임·밑줄·취소선·글자 색(선택이 없으면 다음 입력에 적용), 문단 정렬·줄 간격·왼쪽/오른쪽 여백·첫 줄 들여쓰기/내어쓰기.
입력: 쪽 나누기(Ctrl+Enter), 표 만들기. 표: 줄·칸 추가·지우기(Alt+Enter, Alt+Delete), 셀 합치기(셀을 끌어서 고른 뒤)·나누기, 칸 너비·줄 높이 조절과 같게 하기, 모서리를 끌어 표 크기 조절, Tab으로 다음 칸(마지막 칸에서는 줄 추가), Esc로 표를 개체로 선택한 뒤 Delete로 지우기·Ctrl+C로 복사, 오른쪽 클릭 메뉴.
파일: 새 문서(처음 저장할 때 .hwp 또는 .hwpx 선택), 최근 문서, PDF로 내보내기, 인쇄.
저장: 원래 형식으로만 저장합니다. 편집하지 않은 문서는 다시 쓰지 않고 원본 바이트를 그대로 둡니다. \
저장 결과는 다시 열어 검증한 뒤에만 파일을 교체하고, 기존 파일은 ~/.local/share/hwpedit/backups 에 백업합니다.

아직 지원하지 않음: 다른 프로그램과 서식 있는 복사·붙여넣기(일반 텍스트만), 문단 간격·탭, 셀 테두리·배경, 그림·도형 편집, 머리말·각주 편집.
알려진 제약: HWPX를 편집해 저장하면 엔진이 XML을 다시 씁니다(문단 ID 재번호, 기본값 속성 생략). \
HWP 저장 시 수식 레코드 끝에 2바이트가 추가됩니다. 한컴 프로그램에서의 표시는 아직 검증되지 않았습니다. \
설치되지 않은 글꼴(함초롬 등)은 다른 글꼴로 표시됩니다.";

thread_local! {
    static WINDOWS: std::cell::RefCell<Vec<Rc<Editor>>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn new_window(app: &adw::Application) -> Rc<Editor> {
    let ed = Editor::new(app);
    WINDOWS.with(|w| w.borrow_mut().push(ed.clone()));
    let win = ed.win.clone();
    win.connect_destroy(move |w| {
        // May run while WINDOWS itself is being torn down at exit, or while it is
        // borrowed; dropping the editor must happen after the borrow is released.
        let gone: Vec<Rc<Editor>> = WINDOWS
            .try_with(|ws| match ws.try_borrow_mut() {
                Ok(mut v) => {
                    let (gone, keep) = v.drain(..).partition(|e| e.win == *w);
                    *v = keep;
                    gone
                }
                Err(_) => Vec::new(),
            })
            .unwrap_or_default();
        drop(gone);
    });
    ed.win.present();
    ed
}

/// Open `path` in `ed` if it is empty, otherwise in a new window.
pub fn open_path(ed: &Rc<Editor>, path: &Path) {
    if ed.has_document() {
        let app = ed
            .win
            .application()
            .and_downcast::<adw::Application>()
            .expect("app");
        new_window(&app).load(path);
    } else {
        ed.load(path);
    }
}

fn active_editor(app: &adw::Application) -> Option<Rc<Editor>> {
    let active = app.active_window();
    WINDOWS.with(|ws| {
        let ws = ws.borrow();
        ws.iter()
            .find(|e| active.as_ref() == Some(e.win.upcast_ref()))
            .or(ws.first())
            .cloned()
    })
}

fn open_dialog(app: &adw::Application) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("한글 문서 (*.hwp, *.hwpx)"));
    filter.add_suffix("hwp");
    filter.add_suffix("hwpx");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("문서 열기")
        .filters(&filters)
        .modal(true)
        .build();
    let parent = app.active_window();
    let app = app.clone();
    dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        match active_editor(&app) {
            Some(ed) => open_path(&ed, &path),
            None => new_window(&app).load(&path),
        }
    });
}

fn about(app: &adw::Application) {
    let d = adw::AboutDialog::builder()
        .application_name("HWP 편집기")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::MitX11)
        .comments(
            "Linux용 HWP/HWPX 편집기 (시험판).\n\n\
             본 제품은 한컴의 HWP 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.\n\
             문서 엔진: rhwp (MIT, Edward Kim).",
        )
        .build();
    d.add_credit_section(
        Some("문서 엔진"),
        &["rhwp https://github.com/edwardkim/rhwp"],
    );
    d.present(app.active_window().as_ref());
}

fn main() -> glib::ExitCode {
    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    // with the input-method log on, run on its own: an editor already open would
    // otherwise take the file and this process (and its log) would end at once
    if std::env::var_os("HWPEDIT_IME_LOG").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();

    app.connect_startup(|app| {
        gtk::Window::set_default_icon_name(APP_ID);
        let css = gtk::CssProvider::new();
        css.load_from_string(ribbon::CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            // Above USER: a GTK theme installed in ~/.config/gtk-4.0/gtk.css (USER priority)
            // otherwise repaints the glass panes (dark header bar, filled buttons, faded
            // disabled controls). Only our own selectors win; the theme still styles the rest.
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
            icons::install(&display);
        }
        let open = gio::SimpleAction::new("open", None);
        let a = app.clone();
        open.connect_activate(move |_, _| open_dialog(&a));
        app.add_action(&open);
        let new = gio::SimpleAction::new("new", None);
        let a = app.clone();
        new.connect_activate(
            move |_, _| match active_editor(&a).filter(|e| !e.has_document()) {
                Some(ed) => ed.load_new(),
                None => new_window(&a).load_new(),
            },
        );
        app.add_action(&new);
        let recent = gio::SimpleAction::new("open-recent", Some(glib::VariantTy::STRING));
        let a = app.clone();
        recent.connect_activate(move |_, v| {
            let Some(path) = v.and_then(|v| v.get::<String>()) else {
                return;
            };
            match active_editor(&a) {
                Some(ed) => open_path(&ed, Path::new(&path)),
                None => new_window(&a).load(Path::new(&path)),
            }
        });
        app.add_action(&recent);
        let none = gio::SimpleAction::new("none", None);
        none.set_enabled(false);
        app.add_action(&none);
        let ab = gio::SimpleAction::new("about", None);
        let a = app.clone();
        ab.connect_activate(move |_, _| about(&a));
        app.add_action(&ab);
        let quit = gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| {
            for w in a.windows() {
                w.close();
            }
        });
        app.add_action(&quit);

        app.set_accels_for_action("app.new", &["<Control>n"]);
        app.set_accels_for_action("app.open", &["<Control>o"]);
        app.set_accels_for_action("win.print", &["<Control>p"]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("win.save", &["<Control>s"]);
        app.set_accels_for_action("win.save-as", &["<Control><Shift>s"]);
        app.set_accels_for_action("win.zoom-in", &["<Control>plus", "<Control>equal"]);
        app.set_accels_for_action("win.zoom-out", &["<Control>minus"]);
        app.set_accels_for_action("win.zoom-reset", &["<Control>0"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
        app.set_accels_for_action("win.undo", &["<Control>z"]);
        app.set_accels_for_action("win.redo", &["<Control><Shift>z", "<Control>y"]);
        app.set_accels_for_action("win.bold", &["<Control>b"]);
        app.set_accels_for_action("win.italic", &["<Control>i"]);
        app.set_accels_for_action("win.underline", &["<Control>u"]);
        app.set_accels_for_action("win.find", &["<Control>f"]);
        app.set_accels_for_action("win.char-shape", &["<Alt>l"]);
        for (action, keys) in [
            // Hwp's 글자 크게 is Alt+Shift+E, which here is 수식 (user's choice, 2026-10-31);
            // Hwp 2022 also takes Ctrl+] / Ctrl+[ for 글자 크게 / 작게 (checked in Hwp)
            ("win.font-bigger", &["<Control>bracketright"][..]),
            (
                "win.font-smaller",
                &["<Alt><Shift>r", "<Control>bracketleft"],
            ),
            ("win.spacing-up", &["<Alt><Shift>z"]),
            ("win.spacing-down", &["<Alt><Shift>a"]),
            ("win.align-left", &["<Control><Shift>l"]),
            ("win.align-center", &["<Alt><Shift>c"]),
            ("win.align-right", &["<Alt><Shift>h"]),
            ("win.align-justify", &["<Control><Shift>m"]),
            ("win.delete", &["<Control>e"]),
            ("win.row-col-insert", &["<Alt>Return"]),
            ("win.row-col-delete", &["<Alt>Delete"]),
        ] {
            app.set_accels_for_action(action, keys);
        }
        app.set_accels_for_action("win.para-shape", &["<Alt>t"]);
        // Hwp 2022: 편집 용지 F7, 단 나누기 Ctrl+Shift+Enter, 모양 복사 Alt+C, 문자표 Ctrl+F10
        app.set_accels_for_action("win.page-setup", &["F7"]);
        app.set_accels_for_action("win.column-break", &["<Control><Shift>Return"]);
        app.set_accels_for_action("win.format-copy", &["<Alt>c"]);
        app.set_accels_for_action("win.char-map", &["<Control>F10"]);
        // 수식: Alt+Shift+E (the user's choice; Hwp uses Ctrl+N,M and has 글자 크게 here)
        app.set_accels_for_action("win.equation", &["<Alt><Shift>e"]);
        app.set_accels_for_action("win.replace", &["<Control>h"]);
        app.set_accels_for_action("win.find-next", &["F3"]);
        app.set_accels_for_action("win.find-prev", &["<Shift>F3"]);
    });
    app.connect_activate(|app| {
        if app.active_window().is_none() {
            new_window(app);
        }
    });
    app.connect_open(|app, files, _| {
        for f in files {
            if let Some(p) = f.path() {
                match active_editor(app).filter(|e| !e.has_document()) {
                    Some(ed) => ed.load(&p),
                    None => new_window(app).load(&p),
                }
            }
        }
        if let (Some(dir), Some(ed)) = (std::env::var_os("HWPEDIT_UI_SHOTS"), active_editor(app)) {
            ed.save_ui_shots(dir.into());
        }
    });
    app.run()
}
