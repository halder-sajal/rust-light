fn main() {
    slint_build::compile("ui/app-window.slint").unwrap();

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("ui/app-icon.ico");
        res.compile().expect("failed to embed Windows exe icon");
    }
}
