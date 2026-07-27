use anyhow::{Result, anyhow};

pub struct MacOSKeySimulator;

impl super::KeySimulator for MacOSKeySimulator {
    fn simulate_select_group(&self) -> Result<()> {
        Err(anyhow!("macOS keypress simulation not yet implemented"))
    }
    
    fn simulate_select_mod(&self) -> Result<()> {
        Err(anyhow!("macOS keypress simulation not yet implemented"))
    }
    
    fn check_support(&self) -> Result<(), String> {
        Err("macOS keypress simulation requires assistive access permission, not yet implemented".to_string())
    }
}

pub struct MacOSForegroundDetector;

impl super::ForegroundDetector for MacOSForegroundDetector {
    fn get_foreground_process_name(&self) -> Result<String> {
        Err(anyhow!("macOS foreground detection not yet implemented"))
    }
    
    fn get_cursor_position(&self) -> Result<(i32, i32)> {
        Err(anyhow!("macOS cursor position not yet implemented"))
    }
}
