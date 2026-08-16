use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

pub struct Outcome {
    pub label: String,
    pub result: Result<String, String>,
}

/// `total` stays 0 until the worker knows it, at which point the spinner becomes a bar.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

#[derive(Default)]
pub struct JobRunner {
    running: Option<String>,
    rx: Option<Receiver<Outcome>>,
    progress: Option<Arc<Progress>>,
    pub log: Vec<Outcome>,
}

impl JobRunner {
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn running_label(&self) -> Option<&str> {
        self.running.as_deref()
    }

    /// Ignored if a job is already running. `ctx` is woken on completion, so the result appears
    /// without the user having to move the mouse.
    pub fn spawn<F>(&mut self, ctx: &egui::Context, label: impl Into<String>, f: F)
    where
        F: FnOnce() -> anyhow::Result<String> + Send + 'static,
    {
        if self.busy() {
            return;
        }
        let label = label.into();
        let (tx, rx) = channel();
        self.running = Some(label.clone());
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = f().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Outcome { label, result });
            ctx.request_repaint();
        });
    }

    /// As [`Self::spawn`], but hands the worker a shared [`Progress`] to drive a live bar.
    pub fn spawn_tracked<F>(&mut self, ctx: &egui::Context, label: impl Into<String>, f: F)
    where
        F: FnOnce(Arc<Progress>) -> anyhow::Result<String> + Send + 'static,
    {
        if self.busy() {
            return;
        }
        let label = label.into();
        let (tx, rx) = channel();
        let progress = Arc::new(Progress::default());
        self.running = Some(label.clone());
        self.rx = Some(rx);
        self.progress = Some(progress.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = f(progress).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Outcome { label, result });
            ctx.request_repaint();
        });
    }

    /// `None` until the total is known.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let p = self.progress.as_ref()?;
        let total = p.total.load(Ordering::Relaxed);
        (total > 0).then(|| (p.done.load(Ordering::Relaxed).min(total), total))
    }

    pub fn show_log(&self, ui: &mut egui::Ui) {
        if self.log.is_empty() {
            return;
        }
        egui::CollapsingHeader::new("Activity")
            .default_open(true)
            .show(ui, |ui| {
                for o in &self.log {
                    let (color, msg) = match &o.result {
                        Ok(m) => (egui::Color32::from_rgb(120, 200, 120), m),
                        Err(e) => (egui::Color32::from_rgb(220, 120, 120), e),
                    };
                    ui.label(egui::RichText::new(format!("{}: {msg}", o.label)).color(color));
                }
            });
    }

    /// `true` when a job completed this frame, so the caller can refresh derived state.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(outcome) => {
                self.log.insert(0, outcome);
                self.log.truncate(50);
                self.running = None;
                self.rx = None;
                self.progress = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.running = None;
                self.rx = None;
                self.progress = None;
                true
            }
        }
    }
}
