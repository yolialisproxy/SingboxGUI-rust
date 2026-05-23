use eframe::egui;
use eframe::egui::{CentralPanel, Context, ScrollArea, TextEdit, TopBottomPanel};
use std::collections::VecDeque;
use std::io::{self, BufRead};
use std::process::{Child, Command as StdCommand, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
enum GuiCommand {
    Start { path: String },
    Stop,
}

#[derive(Debug)]
enum Event {
    Log(String),
    ProcessStarted,
    ProcessExited(ExitStatus),
}

struct SingBoxGui {
    // UI State
    logs: VecDeque<String>, // Ring buffer for log lines
    auto_scroll: bool,      // Whether to auto-scroll to bottom on new logs
    is_running: bool,
    singbox_path: String,
    log_filter: String,
    // Cached filtered logs for performance
    filtered_logs_cache: String,
    filtered_logs_dirty: bool,
    prev_log_filter: String,
    // Communication channels
    command_tx: Option<mpsc::Sender<GuiCommand>>,
    event_rx: Option<mpsc::Receiver<Event>>,
    // UI Context
    show_settings: bool,
    show_about: bool,
    settings_path_input: String,
    startup_verified: bool,
    singbox_available: bool,
    // Rendering hints
    needs_repaint: bool, // True when new log lines or status change requires UI refresh
    scroll_to_bottom: bool, // Queue a scroll to bottom on next frame
    // Background
    bg_handle: Option<thread::JoinHandle<()>>,
}

impl SingBoxGui {
    fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel::<GuiCommand>();
        let (event_tx, event_rx) = mpsc::channel::<Event>();

        let bg_handle = thread::spawn(move || {
            background_loop(command_rx, event_tx);
        });

        Self {
            logs: VecDeque::with_capacity(1000),
            auto_scroll: true,
            is_running: false,
            singbox_path: "sing-box".to_string(),
            log_filter: String::new(),
            // Cached filtered logs for performance
            filtered_logs_cache: String::new(),
            filtered_logs_dirty: true,
            prev_log_filter: String::new(),
            command_tx: Some(command_tx),
            event_rx: Some(event_rx),
            show_settings: false,
            show_about: false,
            settings_path_input: "sing-box".to_string(),
            startup_verified: false,
            singbox_available: false,
            needs_repaint: false,
            scroll_to_bottom: false,
            bg_handle: Some(bg_handle),
        }
    }

    fn log(&mut self, message: String) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        let hours = ((secs / 3600) % 24) as u32;
        let minutes = ((secs / 60) % 60) as u32;
        let seconds = (secs % 60) as u32;
        let line = format!("[{:02}:{:02}:{:02}] {}", hours, minutes, seconds, message);
        self.logs.push_back(line);
        // Keep log history bounded (max 1000 lines)
        if self.logs.len() > 1000 {
            self.logs.pop_front();
        }
    }

    fn process_events(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = self.event_rx.as_mut() {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        let mut had_events = false;
        for event in events {
            match event {
                Event::Log(msg) => {
                    self.log(msg);
                    had_events = true;
                }
                Event::ProcessStarted => {
                    self.is_running = true;
                    had_events = true;
                }
                Event::ProcessExited(status) => {
                    if self.is_running {
                        self.log(format!("Process exited with status: {}", status));
                        self.is_running = false;
                        had_events = true;
                    }
                }
            }
        }

        if had_events {
            self.needs_repaint = true;
            if self.auto_scroll {
                self.scroll_to_bottom = true;
            }
        }
    }

    fn start_singbox(&self) {
        if self.is_running {
            return;
        }
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(GuiCommand::Start {
                path: self.singbox_path.clone(),
            });
        }
    }

    fn stop_singbox(&self) {
        if !self.is_running {
            return;
        }
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(GuiCommand::Stop);
        }
    }

    fn verify_singbox_path(&self, path: &str) -> (bool, String) {
        match StdCommand::new(path).arg("--version").output() {
            Ok(output) if output.status.success() => {
                let msg = format!(
                    "Sing-box found: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
                (true, msg)
            }
            _ => {
                let msg = format!("Failed to verify sing-box at {}", path);
                (false, msg)
            }
        }
    }

    fn check_singbox_availability(&mut self) {
        if !self.startup_verified {
            self.startup_verified = true;
            let (available, msg) = self.verify_singbox_path(&self.singbox_path);
            self.singbox_available = available;
            self.log(msg);
            if !self.singbox_available {
                self.log(
                    "Warning: sing-box not found or not executable. Please check the path in Settings."
                        .to_string(),
                );
            }
        }
    }

    fn clear_logs(&mut self) {
        self.logs.clear();
        self.filtered_logs_dirty = true;
    }

    fn build_filtered_logs(&mut self) -> &str {
        // Rebuild if filter changed or logs have been updated since last build
        if self.filtered_logs_dirty || self.log_filter != self.prev_log_filter {
            self.filtered_logs_cache = if self.log_filter.is_empty() {
                self.logs
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                let filter_lower = self.log_filter.to_lowercase();
                let filtered: Vec<String> = self
                    .logs
                    .iter()
                    .filter(|line| line.to_lowercase().contains(&filter_lower))
                    .cloned()
                    .collect();
                filtered
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            self.prev_log_filter = self.log_filter.clone();
            self.filtered_logs_dirty = false;
        }
        &self.filtered_logs_cache
    }
}

impl Drop for SingBoxGui {
    fn drop(&mut self) {
        // Close command channel, signaling background thread to exit
        drop(self.command_tx.take());
        // Join background worker
        if let Some(handle) = self.bg_handle.take() {
            let _ = handle.join();
        }
    }
}

impl eframe::App for SingBoxGui {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.check_singbox_availability();
        self.process_events();

        TopBottomPanel::top("menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Start").clicked() {
                    self.start_singbox();
                }
                if ui.button("Stop").clicked() {
                    self.stop_singbox();
                }
                ui.add_space(10.0);
                if ui.button("⚙️ Settings").clicked() {
                    self.show_settings = true;
                    self.settings_path_input = self.singbox_path.clone();
                }
                ui.add_space(5.0);
                if ui.button("❓ About").clicked() {
                    self.show_about = true;
                }
            });
        });

        TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Status: {}",
                    if self.is_running {
                        "Running"
                    } else {
                        "Stopped"
                    }
                ));
                ui.add_space(10.0);
                ui.label(format!("Path: {}", self.singbox_path));
                ui.add_space(10.0);

                if self.startup_verified {
                    ui.label(format!(
                        "Availability: {}",
                        if self.singbox_available {
                            "✓ Available"
                        } else {
                            "✗ Not found"
                        }
                    ));
                } else {
                    ui.label("Checking availability...");
                }

                ui.add_space(10.0);
                ui.label("💡 Tip: Use Settings to configure sing-box path");
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.checkbox(&mut self.auto_scroll, "Auto-scroll to bottom");
                ui.add_space(5.0);

                // Filter row
                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.text_edit_singleline(&mut self.log_filter);
                    if ui.button("Clear Logs").clicked() {
                        self.clear_logs();
                    }
                });
                ui.label(format!("Log lines: {}", self.logs.len()));
                ui.add_space(5.0);

                // Build display text
                let log_text = self.build_filtered_logs();
                let mut log_buffer = log_text.to_string();

                let scroll_response = ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut log_buffer)
                                .desired_rows(20)
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        )
                    });

                // Auto-scroll
                if self.scroll_to_bottom {
                    scroll_response.inner.scroll_to_me(Some(egui::Align::BOTTOM));
                    self.scroll_to_bottom = false;
                }
            });
        });

        // Settings window
        if self.show_settings {
            egui::Window::new("Settings")
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.label("Sing-box executable path:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.settings_path_input);
                            if ui.button("Check").clicked() {
                                let _ = self.verify_singbox_path(&self.settings_path_input);
                            }
                            if ui.button("Apply").clicked() {
                                if self.verify_singbox_path(&self.settings_path_input).0 {
                                    self.singbox_path = self.settings_path_input.clone();
                                    self.log(format!(
                                        "Settings saved. New path: {}",
                                        self.singbox_path
                                    ));
                                    self.startup_verified = false;
                                }
                                self.show_settings = false;
                            }
                            if ui.button("Cancel").clicked() {
                                self.show_settings = false;
                            }
                        });
                        ui.add_space(10.0);
                        ui.label(
                            "Note: The path can be an absolute path or just the executable name if it's in your PATH."
                        );
                        ui.add_space(10.0);
                        ui.checkbox(&mut self.auto_scroll, "Enable auto-scroll in log view");
                    });
                });
        }

        // About window
        if self.show_about {
            egui::Window::new("About SingboxGUI-Rust")
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.heading("SingboxGUI-Rust");
                        ui.label("A graphical user interface for Sing-box");
                        ui.label("Written in Rust with eframe/egui");
                        ui.add_space(10.0);
                        ui.label("Core features:");
                        ui.label("• Start/stop sing-box process");
                        ui.label("• Real-time logging with timestamps");
                        ui.label("• Live log filter");
                        ui.label("• Configurable sing-box binary path");
                        ui.label("• Persistent log buffer (ring buffer)");
                        ui.label("• Background I/O worker for smooth UI");
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            self.show_about = false;
                        }
                    });
                });
        }

        // Repaint only when new content arrived
        if self.needs_repaint {
            ctx.request_repaint();
            self.needs_repaint = false;
        }
    }
}

/// Background worker: handles sing-box process I/O and lifetime
fn background_loop(command_rx: mpsc::Receiver<GuiCommand>, event_tx: mpsc::Sender<Event>) {
    let mut child: Option<Child> = None;

    loop {
        // Process any pending commands
        match command_rx.try_recv() {
            Ok(GuiCommand::Start { path }) => {
                if child.is_some() {
                    let _ = event_tx.send(Event::Log("Process already running".into()));
                    continue;
                }
                let mut cmd = StdCommand::new(&path);
                cmd.arg("run").stdout(Stdio::piped()).stderr(Stdio::piped());

                match cmd.spawn() {
                    Ok(mut child_process) => {
                        // Spawn stdout reader thread
                        if let Some(stdout) = child_process.stdout.take() {
                            let tx = event_tx.clone();
                            thread::spawn(move || {
                                let reader = io::BufReader::new(stdout);
                                for line in reader.lines().map_while(Result::ok) {
                                    let _ = tx.send(Event::Log(line));
                                }
                            });
                        }

                        // Spawn stderr reader thread
                        if let Some(stderr) = child_process.stderr.take() {
                            let tx = event_tx.clone();
                            thread::spawn(move || {
                                let reader = io::BufReader::new(stderr);
                                for line in reader.lines().map_while(Result::ok) {
                                    let _ = tx.send(Event::Log(line));
                                }
                            });
                        }

                        // Store child handle
                        child = Some(child_process);
                        let _ = event_tx.send(Event::ProcessStarted);
                        let _ = event_tx.send(Event::Log(format!("Started sing-box at {}", path)));
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(Event::Log(format!("Failed to start sing-box: {}", e)));
                    }
                }
            }
            Ok(GuiCommand::Stop) => {
                if let Some(mut child_handle) = child.take() {
                    // Attempt to kill and wait
                    let _ = child_handle.kill();
                    if let Ok(status) = child_handle.wait() {
                        let _ = event_tx.send(Event::ProcessExited(status));
                    } else {
                        let _ = event_tx.send(Event::Log("Failed to wait for child".into()));
                    }
                } else {
                    let _ = event_tx.send(Event::Log("No running process to stop".into()));
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // Channel closed — background worker exits
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        // Check if the child process has exited on its own
        if let Some(child_handle) = child.as_mut() {
            if let Ok(Some(status)) = child_handle.try_wait() {
                let _ = event_tx.send(Event::ProcessExited(status));
                child = None;
            }
        }

        // Avoid busy waiting
        thread::sleep(Duration::from_millis(50));
    }

    // Ensure any remaining child is terminated
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SingboxGUI-Rust",
        native_options,
        Box::new(|_cc| Ok(Box::new(SingBoxGui::new()))),
    )
}