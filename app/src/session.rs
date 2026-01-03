use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::error::Result;
use crate::hook::HookManager;
use crate::os::{Event, EventContext};
use crate::vmi::Vmi;

pub struct Session {
    vmi: Arc<Mutex<Vmi>>,
    hooks: Arc<HookManager>,
    events: Vec<Box<dyn Event>>,
}

impl Session {
    pub fn new(domain_name: &str, json_path: &str, socket_path: &str) -> Result<Self> {
        let vmi = Arc::new(Mutex::new(Vmi::new(domain_name, json_path, socket_path)?));
        let hooks = HookManager::init(vmi.clone())?;
        Ok(Self {
            vmi,
            hooks,
            events: Vec::new(),
        })
    }

    pub fn vmi(&self) -> Arc<Mutex<Vmi>> {
        self.vmi.clone()
    }

    pub fn hooks(&self) -> &Arc<HookManager> {
        &self.hooks
    }

    pub fn add_event<E: Event + 'static>(&mut self, mut event: E) -> Result<()> {
        let ctx = EventContext {
            vmi: &self.vmi,
            hooks: &self.hooks,
        };
        event.enable(&ctx)?;
        self.events.push(Box::new(event));
        Ok(())
    }

    pub fn run(&self, running: Arc<AtomicBool>) -> Result<()> {
        let vmi = self.vmi.clone();
        let running_events = running.clone();

        let event_thread = thread::spawn(move || {
            while running_events.load(Ordering::SeqCst) {
                let res = {
                    let vmi_lock = vmi.lock().unwrap();
                    vmi_lock.events_listen(100)
                };
                if let Err(e) = res {
                    println!("Event thread error: {}", e);
                    break;
                }
            }
        });

        // wait for event thread
        let _ = event_thread.join();
        Ok(())
    }

    /// execute a one-off action
    pub fn execute<A: crate::os::Action<T>, T>(&self, action: A) -> Result<T> {
        let vmi = self.vmi.lock().unwrap();
        action.execute(&vmi)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let ctx = EventContext {
            vmi: &self.vmi,
            hooks: &self.hooks,
        };
        for event in &mut self.events {
            let _ = event.disable(&ctx);
        }

        // explicit shutdown to restore hooks and fix Arc leak
        self.hooks.shutdown();
    }
}
