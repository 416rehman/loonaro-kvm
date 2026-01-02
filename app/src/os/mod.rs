pub mod windows;
// pub mod linux; // future support

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: i32,
    pub name: String,
    pub addr: u64,
}
