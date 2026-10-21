//! The system clipboard's text, through Android's `ClipboardManager` (JNI). From the GridCraft
//! and DesignCraft Android ports.
//!
//! egui-winit has no Android clipboard: copy and paste would stay inside FigCraft. Here text
//! FigCraft copies is put on the system clipboard (`Services::clipboard_set_text`, and egui's copy
//! commands each frame), and Paste reads the system clipboard's text
//! (`Services::clipboard_get_text`, and Ctrl+V in a text field), so text moves to and from the
//! browser, mail and other apps (SVG text copied elsewhere pastes as layers). Layers copied on the
//! canvas stay on FigCraft's own clipboard; pictures aren't exchanged.

use jni::JNIEnv;
use jni::objects::{JObject, JValue};

use crate::file_access::with_activity;
use crate::incoming::string_of;

/// Larger clipboard text is ignored rather than pasted.
const MAX_CHARS: usize = 16 << 20;

/// `ClipboardManager`, or `None` without one (or when Java threw).
fn manager<'local>(env: &mut JNIEnv<'local>, activity: &JObject) -> jni::errors::Result<Option<JObject<'local>>> {
    let name = env.new_string("clipboard")?;
    let cm = env.call_method(activity, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[JValue::Object(&name)])?.l()?;
    Ok((!cm.is_null()).then_some(cm))
}

/// The clipboard's text (what another app copied), if any.
pub fn text() -> Option<String> {
    let text = with_activity("reading the clipboard", |env, activity| {
        let Some(cm) = manager(env, activity)? else { return Ok(None) };
        let clip = env.call_method(&cm, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?.l()?;
        if clip.is_null() || env.call_method(&clip, "getItemCount", "()I", &[])?.i()? < 1 {
            return Ok(None);
        }
        let item = env.call_method(&clip, "getItemAt", "(I)Landroid/content/ClipData$Item;", &[JValue::Int(0)])?.l()?;
        if item.is_null() {
            return Ok(None);
        }
        // Text as text; a link or styled text as its plain form.
        let seq = env.call_method(&item, "coerceToText", "(Landroid/content/Context;)Ljava/lang/CharSequence;", &[JValue::Object(activity)])?.l()?;
        if seq.is_null() {
            return Ok(None);
        }
        let s = env.call_method(&seq, "toString", "()Ljava/lang/String;", &[])?.l()?;
        string_of(env, s)
    })
    .ok()
    .flatten()?;
    (!text.is_empty() && text.len() <= MAX_CHARS).then_some(text)
}

/// Put `text` on the system clipboard.
pub fn set_text(text: &str) {
    let _ = with_activity("copying to the clipboard", |env, activity| {
        let Some(cm) = manager(env, activity)? else { return Ok(()) };
        let label = env.new_string("FigCraft")?;
        let value = env.new_string(text)?;
        let clip = env
            .call_static_method(
                "android/content/ClipData",
                "newPlainText",
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[JValue::Object(&label), JValue::Object(&value)],
            )?
            .l()?;
        env.call_method(&cm, "setPrimaryClip", "(Landroid/content/ClipData;)V", &[JValue::Object(&clip)])?;
        Ok(())
    });
}
