use zed_extension_api as zed;

struct XcodeToolsExtension;

impl zed::Extension for XcodeToolsExtension {
    fn new() -> Self {
        XcodeToolsExtension
    }
}

zed::register_extension!(XcodeToolsExtension);
