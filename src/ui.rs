use std::io::{stdout, Write};

use crossterm::{cursor::MoveTo, execute, terminal::{Clear, ClearType}};
use tokio::sync::mpsc;

use crate::messages::DownloadStats;


pub struct UiManager {
    from_torrent_manager_rx: mpsc::Receiver<DownloadStats>,
}

impl UiManager {
    /// Creates a new UiManager.
    pub fn new(from_torrent_manager_rx: mpsc::Receiver<DownloadStats>) -> Self {
        Self { from_torrent_manager_rx }
    }

    pub async fn run(&mut self) {
        println!("[UiManager] Running.");
        while let Some(stats) = self.from_torrent_manager_rx.recv().await {
            self.draw_dashboard(&stats).await;
        }
    }

    async fn draw_dashboard(&self, stats: &DownloadStats) {
        let mut stdout = stdout();

        let bar_width = 30;

        let filled_width = (stats.progress_percent / 100.0 * bar_width as f32).round() as usize;
        let empty_width = bar_width - filled_width;
        let progress_bar = format!(
            "{}{}",
            "█".repeat(filled_width),
            "░".repeat(empty_width)
        );
        let output = format!(
            "--- BitTorrent Client ---\n\
             File: {}\n\
             Progress: [{}] {:.2}%\n\
             Pieces: {} / {}\n\
             Peers: {}\n\
             Speed: ⬇ {:.2} KB/s | ⬆ {:.2} KB/s\n",
            stats.file_name,
            progress_bar,
            stats.progress_percent,
            stats.pieces_verified,
            stats.total_pieces,
            stats.connected_peers,
            stats.download_speed_kbps,
            stats.upload_speed_kbps
        );
        execute!(stdout, Clear(ClearType::All), MoveTo(0, 0)).unwrap();
        print!("{}", output);
        stdout.flush().unwrap();
    }

}