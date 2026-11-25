//! Languages: English and Hebrew.
//!
//! Every visible string is written in English in the code and translated
//! here at display time (`tr`, `trf`). The widgets mirror the layout for
//! right-to-left languages and use Unicode bidi runs to lay out mixed text.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use unicode_bidi::{bidi_class, BidiClass, Level, ParagraphBidiInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Lang {
    En,
    He,
}

impl Lang {
    pub const ALL: &[Lang] = &[Lang::En, Lang::He];
    /// The language's own name.
    pub fn native(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::He => "עברית",
        }
    }
    pub fn is_rtl(self) -> bool {
        match self {
            Lang::En => false,
            Lang::He => true,
        }
    }

    pub fn from_locale(locale: &str) -> Option<Lang> {
        match locale.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase().as_str() {
            "en" => Some(Lang::En),
            "he" | "iw" => Some(Lang::He),
            _ => None,
        }
    }

    /// From the system's preferred languages.
    pub fn system() -> Lang {
        sys_locale::get_locales().find_map(|locale| Self::from_locale(&locale)).unwrap_or(Lang::En)
    }

    fn messages(self) -> Option<&'static HashMap<&'static str, &'static str>> {
        match self {
            Lang::En => None,
            Lang::He => {
                static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
                Some(MAP.get_or_init(|| HE.iter().copied().collect()))
            }
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn set(language: Lang) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    let language = CURRENT.load(Ordering::Relaxed);
    Lang::ALL.iter().copied().find(|&lang| lang as u8 == language).unwrap_or(Lang::En)
}

pub fn rtl() -> bool {
    lang().is_rtl()
}

/// Translates an English UI string (falls back to the English).
pub fn tr(en: &str) -> Cow<'_, str> {
    Cow::Borrowed(lang().messages().and_then(|messages| messages.get(en).copied()).unwrap_or(en))
}

/// Translates a template with `{}` holes, then fills them in order.
pub fn trf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let t = tr(template);
    let mut out = String::with_capacity(t.len() + 16);
    let mut it = args.iter();
    let mut rest: &str = &t;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(a) = it.next() {
            out.push_str(&a.to_string());
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

pub fn has_rtl_text(s: &str) -> bool {
    !s.is_ascii() && s.chars().any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        _ => c,
    }
}

/// Lay out a line of a logical paragraph in visual run order. epaint shapes
/// each section on its own, so keep RTL letters in logical order for the
/// shaper, while placing spaces and punctuation between the resulting words.
pub fn shape_line(info: &ParagraphBidiInfo<'_>, line: std::ops::Range<usize>, mut append: impl FnMut(&str)) {
    if line.is_empty() {
        return;
    }
    let (levels, runs) = info.visual_runs(line);
    for run in runs {
        let text = &info.text[run.clone()];
        if levels[run.start].is_ltr() {
            append(text);
            continue;
        }

        // Fonts can change at spaces or punctuation. Submit those separately
        // so epaint cannot place two Hebrew words in logical order around them.
        let shapes_rtl = |c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL | BidiClass::NSM);
        let mut end = text.len();
        while end > 0 {
            let rtl = text[..end].chars().next_back().is_some_and(shapes_rtl);
            let start = text[..end].char_indices().rev()
                .find(|&(_, c)| shapes_rtl(c) != rtl)
                .map_or(0, |(i, c)| i + c.len_utf8());
            let part = &text[start..end];
            if rtl {
                append(part);
            } else {
                append(&part.chars().rev().map(mirror).collect::<String>());
            }
            end = start;
        }
    }
}

pub fn paragraph(s: &str) -> ParagraphBidiInfo<'_> {
    ParagraphBidiInfo::new(s, rtl().then(Level::rtl))
}

/// Device details like "Built-in · 3456 × 2234": translate each part.
pub fn tr_parts(s: &str) -> String {
    s.split(" · ").map(|part| {
        if rtl() && part.contains('×') && part.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '×') {
            format!("\u{2066}{part}\u{2069}")
        } else {
            tr(part).into_owned()
        }
    }).collect::<Vec<_>>().join(" · ")
}

/// Device names we generate ourselves ("External display 2").
pub fn tr_device_name(s: &str) -> String {
    for prefix in ["External display ", "Apple display "] {
        if let Some(n) = s.strip_prefix(prefix) {
            return trf(&format!("{prefix}{{}}"), &[&n]);
        }
    }
    tr(s).into_owned()
}

/// English → Hebrew.
static HE: &[(&str, &str)] = &[
    // App, sections, scenes, roles
    ("Studio Record", "Studio Record"),
    ("Sources", "מקורות"),
    ("Output", "פלט"),
    ("Look", "מראה"),
    ("Language", "שפה"),
    ("Settings", "הגדרות"),
    ("Camera", "מצלמה"),
    ("Screen or window", "מסך או חלון"),
    ("Screen", "מסך"),
    ("iPad", "אייפד"),
    ("Link", "קישור"),
    ("Face", "מצלמה"),
    ("Screen + Face", "מסך + מצלמה"),
    ("Link + Face", "קישור + מצלמה"),
    ("iPad + Face", "אייפד + מצלמה"),
    ("iPad beside Face", "אייפד לצד מצלמה"),
    ("Microphone", "מיקרופון"),
    ("Screen & window recording", "הקלטת מסך וחלונות"),
    // Top bar & status bar
    ("Light appearance", "מראה בהיר"),
    ("Dark appearance", "מראה כהה"),
    ("Output window — share this window in a call", "חלון פלט — אפשר לשתף אותו בשיחה"),
    ("Control panel", "פאנל שליטה"),
    ("A small floating panel to switch scenes, record and go live without this window", "פאנל קטן וצף להחלפת סצנות, הקלטה ושידור — בלי החלון הזה"),
    ("Setup check", "בדיקת מוכנות"),
    ("Setup check · {}", "בדיקת מוכנות · {}"),
    ("Camera, microphone, screen and call-camera permissions in one place", "הרשאות מצלמה, מיקרופון, מסך ומצלמה לשיחות — במקום אחד"),
    ("Commands", "פקודות"),
    ("REC  {}", "מקליט  {}"),
    ("PAUSED", "בהמתנה"),
    ("MIC MUTED", "מיקרופון מושתק"),
    ("CAMERA OUT", "מצלמה לשיחות פעילה"),
    (
        "1–{} scenes  ·  ← → slides  ·  from any app: ⌃⌥1–{} scenes, ⌃⌥← → slides, ⌃⌥P pause, ⌃⌥M mute, ⌃⌥R record",
        "\u{2066}1–{}\u{2069} סצנות  ·  \u{2066}← →\u{2069} שקופיות  ·  מכל אפליקציה: \u{2066}⌃⌥1–{}\u{2069} סצנות, \u{2066}⌃⌥← →\u{2069} שקופיות, \u{2066}⌃⌥P\u{2069} המתנה, \u{2066}⌃⌥M\u{2069} השתקה, \u{2066}⌃⌥R\u{2069} הקלטה",
    ),
    (
        "1–{} scenes  ·  ← → slides  ·  from any app: Ctrl+Alt+1–{}, Ctrl+Alt+← →, Ctrl+Alt+P pause, Ctrl+Alt+M mute",
        "\u{2066}1–{}\u{2069} סצנות  ·  \u{2066}← →\u{2069} שקופיות  ·  מכל אפליקציה: \u{2066}Ctrl+Alt+1–{}\u{2069}, \u{2066}Ctrl+Alt+← →\u{2069}, Ctrl+Alt+P המתנה, Ctrl+Alt+M השתקה",
    ),
    // Link bar
    ("‹  Back", "›  הקודם"),
    ("Next  ›", "הבא  ‹"),
    ("Previous slide  (←)", "השקופית הקודמת  (←)"),
    ("Next slide  (→, Space)", "השקופית הבאה  (→, רווח)"),
    ("Paste a link to your slides — Google Slides, PowerPoint, Canva, a PDF…", "הדביקו קישור למצגת — Google Slides,‏ PowerPoint,‏ Canva או PDF…"),
    ("Load", "טען"),
    ("Show this page in the Link scenes", "להציג את הדף בסצנות הקישור"),
    ("Reload the page", "רענון הדף"),
    ("Previous page", "הדף הקודם"),
    ("Next page", "הדף הבא"),
    ("Show the page to click, scroll or sign in", "להציג את הדף כדי ללחוץ, לגלול או להתחבר"),
    // Monitor
    ("PROGRAM  ·  {}", "משודר  ·  {}"),
    ("Video is off", "הווידאו כבוי"),
    ("Turn it back on with Video in the control panel.", "אפשר להדליק אותו שוב בכפתור וידאו בפאנל השליטה."),
    ("The page didn't open", "הדף לא נפתח"),
    ("Opening the page…", "פותח את הדף…"),
    ("No link yet", "עדיין אין קישור"),
    ("Paste a link to your slides in the bar above and press Load.", "הדביקו קישור למצגת בשורה שלמעלה ולחצו על טען."),
    ("No link window", "אין חלון קישור"),
    (
        "Load the link (it opens in your browser), then pick that browser window under Sources → Link.",
        "טענו את הקישור (הוא נפתח בדפדפן), ואז בחרו את חלון הדפדפן תחת מקורות ← קישור.",
    ),
    ("No iPad yet", "עדיין אין אייפד"),
    ("Connect it with a cable and unlock it — it appears automatically.", "חברו אותו בכבל ושחררו את הנעילה — הוא יופיע לבד."),
    ("Pick a capture adapter or display under Sources.", "בחרו מתאם לכידה או מסך תחת מקורות."),
    ("No camera chosen", "לא נבחרה מצלמה"),
    ("No screen or window chosen", "לא נבחר מסך או חלון"),
    ("No link chosen", "לא נבחר קישור"),
    ("Choose one under Sources.", "אפשר לבחור תחת מקורות."),
    ("Starts when Studio Record opens", "יופעל כש־Studio Record ייפתח"),
    ("{} disconnected", "{} — החיבור נותק"),
    ("Plug it back in and it resumes on its own.", "חברו אותו שוב והוא ימשיך לבד."),
    ("Camera access is off", "הגישה למצלמה כבויה"),
    ("Allow Studio Record in Privacy settings.", "אפשרו ל־Studio Record בהגדרות הפרטיות."),
    ("Screen recording is off", "הקלטת המסך כבויה"),
    ("Allow Studio Record in Privacy settings, then reopen it.", "אפשרו ל־Studio Record בהגדרות הפרטיות, ואז פתחו אותו מחדש."),
    ("{} isn't available", "לא ניתן להשתמש ב־{}"),
    ("Starting…", "מתחיל…"),
    // Link card
    ("No link loaded", "לא נטען קישור"),
    ("Live  ·  {}×{}  ·  {} fps", "פעיל  ·  \u{2066}{}×{}\u{2069}  ·  \u{2066}{} fps\u{2069}"),
    ("Live  ·  {}  ·  {} fps", "פעיל  ·  {}  ·  \u{2066}{} fps\u{2069}"),
    ("Waiting for the page…", "ממתין לדף…"),
    ("Hide the page window", "להסתיר את חלון הדף"),
    ("Show page to click or sign in", "להציג את הדף כדי ללחוץ או להתחבר"),
    (
        "Paste a link to your slides in the bar above the preview. Next and Back move the slides — also from the control panel and ⌃⌥← →.",
        "הדביקו קישור למצגת בשורה שמעל התצוגה. הבא והקודם מעבירים שקופיות — גם מפאנל השליטה ובקיצור ⌃⌥← →.",
    ),
    ("Recent links", "קישורים אחרונים"),
    // Setup check
    ("Everything Studio Record needs, with one button to fix each. It updates live.", "כל מה ש־Studio Record צריך, עם כפתור אחד לתיקון כל דבר. מתעדכן בזמן אמת."),
    ("Done", "סיום"),
    ("Open Studio Record as an app first", "קודם צריך לפתוח את Studio Record כאפליקציה"),
    (
        "macOS only shows the Allow dialogs to a real app. This installs Studio Record in your Applications folder and reopens it there.",
        "macOS מציג את חלונות האישור רק לאפליקציה אמיתית. הכפתור יתקין את Studio Record בתיקיית היישומים ויפתח אותו משם.",
    ),
    ("Reopen as app", "פתיחה כאפליקציה"),
    ("Couldn't reopen as an app. Build it with the bundle script and open Studio Record.app.", "לא הצלחנו לפתוח כאפליקציה. בנו אותה עם סקריפט האריזה ופתחו את Studio Record.app."),
    ("Working — sharing {}", "פועל — משתף את {}"),
    ("Working — {} · {}×{} · {} fps", "פועל — {} · \u{2066}{}×{}\u{2069} · \u{2066}{} fps\u{2069}"),
    ("Allowed", "מאושר"),
    ("Not asked yet — press Allow and the system asks once.", "עוד לא נשאלתם — לחצו על אישור והמערכת תשאל פעם אחת."),
    ("Not allowed yet. Allow, switch Studio Record on in Settings, then Reopen.", "עדיין לא מאושר. לחצו אישור, הפעילו את Studio Record בהגדרות ולחצו פתיחה מחדש."),
    ("Turned off for Studio Record. Reset asks again from scratch.", "כבוי עבור Studio Record. איפוס ישאל שוב מההתחלה."),
    ("Blocked by this computer's management profile.", "חסום על ידי פרופיל הניהול של המחשב."),
    ("Allow", "אישור"),
    ("Reopen", "פתיחה מחדש"),
    ("Screen recording applies after a restart", "הקלטת מסך נכנסת לתוקף אחרי פתיחה מחדש"),
    ("Reset & ask", "איפוס ושאלה"),
    ("Clears a stuck approval and shows the system question again", "מנקה אישור תקוע ומציג שוב את שאלת המערכת"),
    (
        "In Settings, switch Studio Record on under Screen & System Audio Recording, then press Reopen.",
        "בהגדרות, הפעילו את Studio Record תחת הקלטת מסך ושמע מערכת, ואז לחצו פתיחה מחדש.",
    ),
    ("macOS is asking about the {} — answer the dialog.", "macOS שואל לגבי {} — ענו בחלון שנפתח."),
    ("{} permission reset — answer the question that just appeared.", "ההרשאה ל־{} אופסה — ענו על השאלה שהופיעה."),
    ("Couldn't reset ({}). Opening Settings instead.", "האיפוס לא הצליח ({}). פותח את ההגדרות במקום."),
    ("Live — in Meet, Zoom or Teams choose the camera \"{}\".", "פעיל — ב־Meet, ‏Zoom או Teams בחרו במצלמה \"{}\"."),
    ("Driver ready (\"{}\"). Turn it on, then pick it as your camera in the call.", "מנהל ההתקן מוכן (\"{}\"). הפעילו, ובחרו בו כמצלמה בשיחה."),
    (
        "One-time: install the free OBS Studio, open it once, click Start Virtual Camera and Allow, then quit OBS.",
        "פעם אחת: התקינו את OBS Studio החינמי, פתחו אותו, לחצו Start Virtual Camera ואשרו, ואז סגרו את OBS.",
    ),
    ("One-time: install the free OBS Studio (it adds \"OBS Virtual Camera\").", "פעם אחת: התקינו את OBS Studio החינמי (הוא מוסיף \"OBS Virtual Camera\")."),
    ("Camera for calls", "מצלמה לשיחות"),
    ("Turn off", "כיבוי"),
    ("Turn on", "הפעלה"),
    ("Get driver", "הורדת מנהל התקן"),
    ("Live — choose it as the microphone in your call (Mute here mutes the call too).", "פעיל — בחרו בו כמיקרופון בשיחה (השתקה כאן משתיקה גם את השיחה)."),
    ("Optional. \"{}\" found — turn on to send your voice through Studio Record.", "רשות. נמצא \"{}\" — הפעילו כדי להעביר את הקול דרך Studio Record."),
    ("Optional. Calls can use your microphone directly. For one Mute button for everything, add a free virtual mic.", "רשות. השיחה יכולה להשתמש במיקרופון ישירות. לכפתור השתקה אחד לכל דבר, הוסיפו מיקרופון וירטואלי חינמי."),
    ("Audio for calls", "שמע לשיחות"),
    ("Get virtual mic", "הורדת מיקרופון וירטואלי"),
    ("Choose your language", "בחרו שפה"),
    // Source cards
    ("Video on / off (the camera light goes out)", "וידאו פועל / כבוי (נורית המצלמה נכבית)"),
    ("Mirror the camera", "היפוך תמונת המצלמה"),
    ("Disconnected", "מנותק"),
    ("None", "ללא"),
    ("iPad & iPhone (cable)", "אייפד ואייפון (בכבל)"),
    ("Displays (Sidecar, AirPlay receiver)", "מסכים (Sidecar, מקלט AirPlay)"),
    ("Capture adapters & cameras", "מתאמי לכידה ומצלמות"),
    ("Single windows (only that window is shared)", "חלונות בודדים (משותף רק החלון שנבחר)"),
    ("Nothing found yet", "עדיין לא נמצא דבר"),
    ("Waiting for your permission — it starts by itself once allowed.", "ממתין לאישור שלכם — יתחיל לבד ברגע שתאשרו."),
    ("Fix in Setup check", "תיקון בבדיקת מוכנות"),
    ("Allow screen recording in Setup check to share your screen or a single window.", "אשרו הקלטת מסך בבדיקת המוכנות כדי לשתף את המסך או חלון בודד."),
    ("Load your link in the bar above — it opens in your browser. Then choose that browser window here.", "טענו את הקישור בשורה שלמעלה — הוא ייפתח בדפדפן. ואז בחרו כאן את חלון הדפדפן."),
    (
        "Plug the iPad in with a USB cable, unlock it and tap Trust. It shows up here on its own — no app needed on the iPad.",
        "חברו את האייפד בכבל USB, שחררו את הנעילה והקישו \"סמוך\". הוא יופיע כאן לבד — אין צורך באפליקציה על האייפד.",
    ),
    (
        "Connect the iPad through an HDMI capture adapter (it appears as a camera), or mirror it to an AirPlay receiver app and choose that display.",
        "חברו את האייפד דרך מתאם לכידה HDMI (הוא יופיע כמצלמה), או שקפו אותו לאפליקציית מקלט AirPlay ובחרו את המסך הזה.",
    ),
    ("Not chosen", "לא נבחר"),
    ("Disconnected — waiting for it", "מנותק — ממתין לחיבור"),
    ("Needs your permission", "נדרש אישור שלכם"),
    ("Off", "כבוי"),
    ("Built-in", "מובנה"),
    ("Continuity Camera", "מצלמת Continuity"),
    ("Virtual camera", "מצלמה וירטואלית"),
    ("USB camera", "מצלמת USB"),
    ("Built-in Display", "המסך המובנה"),
    ("Main display", "מסך ראשי"),
    ("External", "חיצוני"),
    ("Window", "חלון"),
    ("iPad screen", "מסך אייפד"),
    ("iPhone screen", "מסך אייפון"),
    ("cable", "בכבל"),
    ("Display", "מסך"),
    ("External display {}", "מסך חיצוני {}"),
    ("Apple display {}", "מסך Apple ‏{}"),
    // Output card
    ("Stop recording   {}", "עצירת הקלטה   {}"),
    ("Saving recording…", "שומר את ההקלטה…"),
    ("Start recording", "התחלת הקלטה"),
    ("Stop recording", "עצירת הקלטה"),
    ("Checking the recording engine…", "בודק את מנוע ההקלטה…"),
    ("Recording uses FFmpeg, a free video engine. It isn't on this computer yet.", "ההקלטה משתמשת ב־FFmpeg, מנוע וידאו חינמי. הוא עדיין לא מותקן במחשב."),
    ("Set up recording (≈80 MB)", "הגדרת הקלטה (כ־80MB)"),
    ("Setup failed: {}", "ההגדרה נכשלה: {}"),
    ("Record your voice with the video", "להקליט את הקול יחד עם הווידאו"),
    ("System default", "ברירת המחדל של המערכת"),
    ("Unmute microphone", "ביטול השתקת המיקרופון"),
    ("Mute microphone", "השתקת המיקרופון"),
    ("Teams, Meet and Zoom see this picture", "Teams,‏ Meet ו־Zoom רואים את התמונה הזו"),
    ("Connecting…", "מתחבר…"),
    ("Live. In your call, pick the camera \"{}\".", "פעיל. בשיחה, בחרו במצלמה \"{}\"."),
    ("Get the free camera driver", "הורדת מנהל ההתקן החינמי למצלמה"),
    (
        "After installing OBS, open it once and click Start Virtual Camera to approve the driver, then quit OBS.",
        "אחרי התקנת OBS, פתחו אותו פעם אחת ולחצו Start Virtual Camera כדי לאשר את מנהל ההתקן, ואז סגרו את OBS.",
    ),
    ("Studio Record connects automatically as soon as it's there.", "Studio Record יתחבר אוטומטית ברגע שהוא יהיה זמין."),
    ("Automatic", "אוטומטי"),
    ("Your voice through Studio Record, with Mute", "הקול שלכם דרך Studio Record, עם השתקה"),
    ("Live. In your call, pick the microphone \"{}\".", "פעיל. בשיחה, בחרו במיקרופון \"{}\"."),
    ("Open output window to share", "פתיחת חלון פלט לשיתוף"),
    ("A clean window with just the picture — choose it when you share a window in a call.", "חלון נקי עם התמונה בלבד — בחרו בו כשמשתפים חלון בשיחה."),
    ("Quality", "איכות"),
    ("Saves to", "נשמר אל"),
    ("Open the recordings folder", "פתיחת תיקיית ההקלטות"),
    // Look card
    ("Face corner", "פינת המצלמה"),
    ("Face size", "גודל המצלמה"),
    ("Face shape", "צורת המצלמה"),
    ("Rounded", "מעוגל"),
    ("Circle", "עיגול"),
    ("Switch", "מעבר"),
    ("Cut", "חיתוך"),
    ("Quick fade", "דהייה קצרה"),
    // Palette
    ("Type a command…", "הקלידו פקודה…"),
    ("No command matches.", "אין פקודה מתאימה."),
    ("Switch to {}", "מעבר אל {}"),
    ("Turn camera for calls off", "כיבוי המצלמה לשיחות"),
    ("Turn camera for calls on", "הפעלת המצלמה לשיחות"),
    ("Open output window", "פתיחת חלון הפלט"),
    ("Toggle mini controls", "הצגה או הסתרה של פאנל השליטה"),
    ("Open recordings folder", "פתיחת תיקיית ההקלטות"),
    ("Mirror camera", "היפוך המצלמה"),
    ("Switch light / dark", "מעבר בין בהיר לכהה"),
    ("Look for cameras and iPads again", "חיפוש מחדש של מצלמות ואייפדים"),
    ("Change language to {}", "שינוי שפה ל־{}"),
    ("Open Setup check", "פתיחת בדיקת מוכנות"),
    // Windows & control panel
    ("Studio Record — Output", "Studio Record — פלט"),
    ("Studio Record — Control panel", "Studio Record — פאנל שליטה"),
    ("Studio Record — Link", "Studio Record — קישור"),
    ("Live", "בשידור"),
    ("Connecting", "מתחבר"),
    ("Go live", "שידור"),
    ("Camera for calls — Meet, Zoom and Teams see this picture", "מצלמה לשיחות — Meet,‏ Zoom ו־Teams רואים את התמונה הזו"),
    ("Record", "הקלטה"),
    ("Start / stop recording", "התחלה / עצירה של הקלטה"),
    ("Paused", "בהמתנה"),
    ("Pause", "המתנה"),
    ("Pause: calls see a still picture and the recording pauses", "המתנה: בשיחה רואים תמונה קפואה וההקלטה נעצרת זמנית"),
    ("Muted", "מושתק"),
    ("Mic", "מיקרופון"),
    ("Mute / unmute your microphone", "השתקה / ביטול השתקה של המיקרופון"),
    ("Video", "וידאו"),
    ("Video off", "וידאו כבוי"),
    ("Turn your camera on / off", "הדלקה / כיבוי של המצלמה"),
    ("Back", "הקודם"),
    ("Previous slide", "השקופית הקודמת"),
    ("Next", "הבא"),
    ("Next slide", "השקופית הבאה"),
    ("Studio", "סטודיו"),
    ("Open the studio window", "פתיחת חלון הסטודיו"),
    // Toasts & messages
    ("{} connected — ready in the iPad scenes.", "{} מחובר — מוכן בסצנות האייפד."),
    ("Install a virtual microphone first (BlackHole on Mac, VB-CABLE on Windows) — see Setup check.", "קודם צריך להתקין מיקרופון וירטואלי (BlackHole במק, VB-CABLE בווינדוס) — ראו בדיקת מוכנות."),
    ("Recording needs the free FFmpeg engine — set it up in Output.", "ההקלטה צריכה את מנוע FFmpeg החינמי — אפשר להגדיר אותו בפלט."),
    ("Saved \"{}\"", "נשמר \"{}\""),
    ("Show in Finder", "הצגה ב־Finder"),
    ("Show file", "הצגת הקובץ"),
    // Capture states (made on capture threads)
    ("Waiting for the first picture — is the window minimised?", "ממתין לתמונה הראשונה — האם החלון ממוזער?"),
    ("The device stopped. It may be in use by another app or was unplugged.", "ההתקן הפסיק. ייתכן שאפליקציה אחרת משתמשת בו או שהוא נותק."),
    ("Waiting for the iPad — unlock it and tap Trust if asked.", "ממתין לאייפד — שחררו את הנעילה והקישו \"סמוך\" אם תתבקשו."),
    ("Waiting for the screen picture…", "ממתין לתמונת המסך…"),
    ("Waiting for the camera…", "ממתין למצלמה…"),
    ("Waiting for the window…", "ממתין לחלון…"),
    ("No picture from the camera — is another app holding it?", "אין תמונה מהמצלמה — אולי אפליקציה אחרת משתמשת בה?"),
    ("This device is no longer connected.", "ההתקן כבר לא מחובר."),
    ("This device is busy in another app.", "ההתקן תפוס באפליקציה אחרת."),
    ("Couldn't attach to this device.", "לא הצלחנו להתחבר להתקן."),
    ("This display is no longer available.", "המסך הזה כבר לא זמין."),
    ("This window was closed.", "החלון הזה נסגר."),
    ("Screen capture didn't start", "לכידת המסך לא התחילה"),
    ("Screen recording isn't allowed", "הקלטת מסך לא מאושרת"),
    ("The window is minimised or hidden.", "החלון ממוזער או מוסתר."),
    ("Unknown device", "התקן לא מוכר"),
    ("That doesn't look like a web address.", "זו לא נראית כמו כתובת אינטרנט."),
    // Call camera / recorder / bridge
    (
        "No virtual camera driver found. Install the free OBS Studio once — it adds \"OBS Virtual Camera\", which Studio Record drives for you.",
        "לא נמצא מנהל התקן למצלמה וירטואלית. התקינו פעם אחת את OBS Studio החינמי — הוא מוסיף \"OBS Virtual Camera\" ש־Studio Record מפעיל בשבילכם.",
    ),
    ("Downloading the recording engine (about 80 MB)…", "מוריד את מנוע ההקלטה (כ־80MB)…"),
    ("Unpacking…", "מחלץ…"),
    ("Download failed — check the internet connection.", "ההורדה נכשלה — בדקו את החיבור לאינטרנט."),
    ("Couldn't unpack the download.", "לא הצלחנו לחלץ את ההורדה."),
    ("The download didn't contain FFmpeg.", "ההורדה לא כללה את FFmpeg."),
    ("The downloaded recording engine didn't start.", "מנוע ההקלטה שהורד לא הופעל."),
    ("Finding the right download…", "מחפש את ההורדה המתאימה…"),
    ("Starting…", "מתחיל…"),
    ("The recording engine couldn't write the video. See last-recording.log.", "מנוע ההקלטה לא הצליח לכתוב את הווידאו. פרטים ב־last-recording.log."),
    ("No microphone found.", "לא נמצא מיקרופון."),
    ("Apple hardware encoder", "מקודד חומרה של Apple"),
    ("NVIDIA hardware encoder", "מקודד חומרה של NVIDIA"),
    ("AMD hardware encoder", "מקודד חומרה של AMD"),
    ("Windows hardware encoder", "מקודד חומרה של Windows"),
    ("x264 (software)", "x264 (תוכנה)"),
    ("Basic encoder", "מקודד בסיסי"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_locale_matches_supported_language_codes() {
        assert_eq!(Lang::from_locale("en-GB"), Some(Lang::En));
        assert_eq!(Lang::from_locale("he_IL.UTF-8"), Some(Lang::He));
        assert_eq!(Lang::from_locale("iw-IL"), Some(Lang::He));
        assert_eq!(Lang::from_locale("fr-FR"), None);
    }
}
