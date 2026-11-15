//! Drag and drop for the file shelf: the pill as a drop target for files (`register`) and a drag
//! source that hands files to any other app (`drag_out`).
//!
//! Only real files are taken (`CF_HDROP`). Windows tells a window about a drag only once the
//! pointer is over it, so the drop zone is the pill itself (see `window::set_region`).

use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL, S_OK};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IBindCtx, IDataObject,
    TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_NONE, DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_NODEREFERENCELINKS, FileOpenDialog, HDROP,
    IFileOpenDialog, SHCreateShellItemArrayFromIDLists, SHParseDisplayName, SIGDN_FILESYSPATH,
};
use windows::core::{BOOL, HRESULT, PCWSTR, Ref, Result, implement, w};

/// The format a file drop carries: a list of paths.
fn hdrop() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn has_files(data: &IDataObject) -> bool {
    unsafe { data.QueryGetData(&hdrop()) == S_OK }
}

/// Every path in an `HDROP`, folders included.
fn paths_from_hdrop(h: HDROP) -> Vec<String> {
    unsafe {
        let n = DragQueryFileW(h, u32::MAX, None);
        (0..n)
            .filter_map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(h, i, None) as usize + 1];
                let got = DragQueryFileW(h, i, Some(&mut buf)) as usize;
                (got > 0).then(|| String::from_utf16_lossy(&buf[..got]))
            })
            .collect()
    }
}

fn paths(data: &IDataObject) -> Vec<String> {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop()) else { return Vec::new() };
        let list = paths_from_hdrop(HDROP(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        list
    }
}

/// Asks the user for files with the standard open dialog (modal over `owner`). Empty if cancelled.
/// Files only: a folder is added by dropping it. Shortcuts are taken as themselves, like a drop.
pub fn pick_files(owner: HWND) -> Vec<String> {
    unsafe {
        let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return Vec::new() };
        let Ok(opts) = dlg.GetOptions() else { return Vec::new() };
        let _ = dlg.SetOptions(opts | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM | FOS_NODEREFERENCELINKS);
        let _ = dlg.SetTitle(w!("Add to shelf"));
        if dlg.Show(Some(owner)).is_err() {
            return Vec::new(); // cancelled
        }
        let Ok(items) = dlg.GetResults() else { return Vec::new() };
        let n = items.GetCount().unwrap_or(0);
        (0..n)
            .filter_map(|i| {
                let name = items.GetItemAt(i).ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
                let path = name.to_string().ok();
                CoTaskMemFree(Some(name.0.cast()));
                path
            })
            .collect()
    }
}

/// The pill's drop target. It only reports to the app; the app decides what a drop means.
#[implement(IDropTarget)]
struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<'_, IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let take = data.as_ref().is_some_and(has_files) && crate::app::with_app(|a| a.on_drag(true)).unwrap_or(false);
        unsafe { *effect = if take { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let take = crate::app::with_app(|a| a.drag_accepts()).unwrap_or(false);
        unsafe { *effect = if take { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        crate::app::with_app(|a| a.on_drag(false));
        Ok(())
    }

    fn Drop(&self, data: Ref<'_, IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let list = data.as_ref().map(paths).unwrap_or_default();
        let took = crate::app::with_app(|a| a.on_drop(list)).unwrap_or(false);
        unsafe { *effect = if took { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }
}

pub fn register(hwnd: HWND) {
    let target: IDropTarget = DropTarget.into();
    if let Err(e) = unsafe { RegisterDragDrop(hwnd, &target) } {
        crate::log(&format!("drag and drop is unavailable: {e}"));
    }
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

/// The drag source: the drag ends when the button is released, or on Esc.
#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags `files` out of the pill into whatever the user drops them on. Blocks until the button is
/// released; the files are offered for copying only, so the originals never move. Returns true if
/// something took them.
pub fn drag_out(files: &[String]) -> bool {
    unsafe {
        let wide: Vec<Vec<u16>> = files.iter().map(|f| f.encode_utf16().chain(Some(0)).collect()).collect();
        let pidls: Vec<*const ITEMIDLIST> = wide
            .iter()
            .filter_map(|w| {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None).ok().map(|_| pidl as *const ITEMIDLIST)
            })
            .collect();
        // an item array can mix folders, and its data object is the one Explorer drags (it carries CF_HDROP)
        let data: Result<IDataObject> = if pidls.is_empty() {
            Err(S_OK.into())
        } else {
            SHCreateShellItemArrayFromIDLists(&pidls).and_then(|items| items.BindToHandler(None::<&IBindCtx>, &BHID_DataObject))
        };
        let took = match data {
            Ok(data) => {
                let source: IDropSource = DropSource.into();
                let mut effect = DROPEFFECT_NONE;
                DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) == DRAGDROP_S_DROP && effect != DROPEFFECT_NONE
            }
            Err(e) => {
                crate::log(&format!("could not start a drag: {e}"));
                false
            }
        };
        for p in pidls {
            CoTaskMemFree(Some(p.cast()));
        }
        took
    }
}
