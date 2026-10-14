use lisnap_core::PixelSize;

use super::annotation::RectI;

pub(crate) const FEEDBACK_SIZE: PixelSize = PixelSize { width: 520, height: 88 };

#[derive(Debug, Clone)]
pub(crate) struct EditorFeedback {
    pub(crate) copy: bool,
    pub(crate) failed: bool,
    pub(crate) detail: String,
}

impl EditorFeedback {
    pub(crate) fn title(&self, chinese: bool) -> &'static str {
        match (self.copy, self.failed, chinese) {
            (false, false, true) => "截图已保存",
            (true, false, true) => "截图已复制到剪贴板",
            (false, true, true) => "保存失败，请重试",
            (true, true, true) => "复制失败，请重试",
            (false, false, false) => "SAVED",
            (true, false, false) => "COPIED TO CLIPBOARD",
            (false, true, false) => "SAVE FAILED - RETRY",
            (true, true, false) => "COPY FAILED - RETRY",
        }
    }

    pub(crate) fn display_detail(&self, max_chars: usize) -> String {
        let chars: Vec<_> = self.detail.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
        if chars.len() <= max_chars {
            return chars.into_iter().collect();
        }
        if max_chars <= 3 {
            return chars.into_iter().take(max_chars).collect();
        }
        let keep = max_chars.saturating_sub(3);
        if self.failed {
            format!("{}...", chars[..keep].iter().collect::<String>())
        } else {
            // Keep the filename visible even when the output directory is long.
            format!("...{}", chars[chars.len() - keep..].iter().collect::<String>())
        }
    }
}

pub(crate) fn feedback_layout(screen: PixelSize) -> RectI {
    let screen_width = i32::try_from(screen.width).unwrap_or(i32::MAX);
    let screen_height = i32::try_from(screen.height).unwrap_or(i32::MAX);
    let width = (FEEDBACK_SIZE.width as i32).min(screen_width);
    let height = (FEEDBACK_SIZE.height as i32).min(screen_height);
    RectI { x: (screen_width - width) / 2, y: 24.min(screen_height - height), width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_paths_keep_filename_and_errors_keep_reason_without_splitting_unicode() {
        let mut feedback = EditorFeedback { copy: false, failed: false, detail: "/很长的路径/截图.png".into() };
        assert_eq!(feedback.display_detail(9), "...截图.png");
        feedback.failed = true;
        feedback.detail = "无法保存\n权限不足，请检查路径".into();
        assert_eq!(feedback.display_detail(9), "无法保存 权...");
        assert_eq!(feedback.title(false), "SAVE FAILED - RETRY");
        assert_eq!(feedback.title(true), "保存失败，请重试");
    }

    #[test]
    fn feedback_stays_on_screen_including_small_outputs() {
        for screen in [PixelSize { width: 1920, height: 1080 }, PixelSize { width: 10, height: 10 }] {
            let rect = feedback_layout(screen);
            assert!(rect.x >= 0 && rect.y >= 0);
            assert!(rect.x + rect.width <= screen.width as i32);
            assert!(rect.y + rect.height <= screen.height as i32);
        }
    }
}
