use std::ffi::{CStr, c_void};

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
    use windows::core::PCSTR;

    #[link(name = "opengl32")]
    unsafe extern "system" {
        fn wglGetProcAddress(name: PCSTR) -> *const c_void;
    }

    fn opengl32() -> HMODULE {
        static MODULE: OnceLock<usize> = OnceLock::new();
        HMODULE(*MODULE.get_or_init(|| unsafe {
            GetModuleHandleA(windows::core::s!("opengl32.dll"))
                .unwrap_or_else(|_| {
                    LoadLibraryA(windows::core::s!("opengl32.dll")).unwrap_or_default()
                })
                .0 as usize
        }) as *mut _)
    }

    fn is_wgl_ptr_valid(ptr: *const c_void) -> bool {
        let value = ptr as usize;
        !ptr.is_null() && value > 3 && value != usize::MAX
    }

    pub fn load_gl_symbol(name: &CStr) -> *const c_void {
        unsafe {
            let ptr = wglGetProcAddress(PCSTR(name.as_ptr() as _));
            if is_wgl_ptr_valid(ptr) {
                return ptr;
            }

            GetProcAddress(opengl32(), PCSTR(name.as_ptr() as _))
                .map(|symbol| symbol as *const c_void)
                .unwrap_or(std::ptr::null())
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use std::os::raw::{c_char, c_int};
    use std::sync::OnceLock;

    #[link(name = "EGL")]
    unsafe extern "C" {
        fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    }

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_LAZY: c_int = 0x1;

    fn gles_handle() -> *mut c_void {
        static HANDLE: OnceLock<usize> = OnceLock::new();
        *HANDLE.get_or_init(|| unsafe { dlopen(c"libGLESv2.so".as_ptr(), RTLD_LAZY) as usize })
            as *mut c_void
    }

    fn egl_handle() -> *mut c_void {
        static HANDLE: OnceLock<usize> = OnceLock::new();
        *HANDLE.get_or_init(|| unsafe { dlopen(c"libEGL.so".as_ptr(), RTLD_LAZY) as usize })
            as *mut c_void
    }

    pub fn load_gl_symbol(name: &CStr) -> *const c_void {
        unsafe {
            let ptr = eglGetProcAddress(name.as_ptr());
            if !ptr.is_null() {
                return ptr;
            }

            let gles = dlsym(gles_handle(), name.as_ptr());
            if !gles.is_null() {
                return gles;
            }

            dlsym(egl_handle(), name.as_ptr()) as *const c_void
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use super::*;
    use std::os::raw::{c_char, c_int};
    use std::sync::OnceLock;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_LAZY: c_int = 0x1;

    #[cfg(target_os = "macos")]
    const OPENGL_FRAMEWORK: &CStr = c"/System/Library/Frameworks/OpenGL.framework/OpenGL";
    #[cfg(target_os = "ios")]
    const OPENGL_FRAMEWORK: &CStr = c"/System/Library/Frameworks/OpenGLES.framework/OpenGLES";

    fn framework_handle() -> *mut c_void {
        static HANDLE: OnceLock<usize> = OnceLock::new();
        *HANDLE.get_or_init(|| unsafe { dlopen(OPENGL_FRAMEWORK.as_ptr(), RTLD_LAZY) as usize })
            as *mut c_void
    }

    pub fn load_gl_symbol(name: &CStr) -> *const c_void {
        unsafe { dlsym(framework_handle(), name.as_ptr()) as *const c_void }
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
mod imp {
    use super::*;

    pub fn load_gl_symbol(_name: &CStr) -> *const c_void {
        std::ptr::null()
    }
}

pub(crate) use imp::load_gl_symbol;
