use std::error::Error;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::spawn;

use gio::prelude::{ApplicationExt, ApplicationExtManual};
use glib::{
    clone, home_dir, user_config_dir, Continue, GString, KeyFile, KeyFileFlags, MainContext,
    Sender, PRIORITY_DEFAULT,
};
use gtk::{
    prelude::{
        BoxExt, DialogExt, EntryExt, FileChooserExt, GridExt, GtkApplicationExt, GtkWindowExt,
        NativeDialogExt, ProgressBarExt, RangeExt, SpinButtonExt, WidgetExt,
    },
    Adjustment, Application, ButtonsType, Dialog, DialogFlags, Entry, EntryIconPosition,
    FileChooserAction, FileChooserNative, Grid, Label, MessageDialog, MessageType, Orientation,
    ProgressBar, ResponseType, Scale, SpinButton, NONE_WINDOW,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

fn main() {
    let application = Application::new(None, Default::default());

    application.connect_activate(clone!(@strong application => move |_| {
        show_dialog(&application);
    }));

    application.run();
}

fn show_dialog(application: &Application) {
    let settings = KeyFile::new();

    let mut settings_file = user_config_dir();
    settings_file.push("resize_jpeg.ini");

    let _ = settings.load_from_file(&settings_file, KeyFileFlags::empty());

    let dialog = Dialog::with_buttons(
        Some("Resize JPEG"),
        NONE_WINDOW,
        DialogFlags::empty(),
        &[("Ok", ResponseType::Ok), ("Cancel", ResponseType::Cancel)],
    );

    let input_dir = Entry::new();

    if let Ok(path) = settings.string("paths", "input_dir") {
        input_dir.set_text(&path);
    }

    input_dir.set_icon_from_icon_name(EntryIconPosition::Secondary, Some("document-open"));
    input_dir.set_icon_activatable(EntryIconPosition::Secondary, true);

    input_dir.connect_icon_press(clone!(@strong dialog => move |entry, _, _| {
        let chooser = FileChooserNative::new(
            Some("Select input directory"),
            Some(&dialog),
            FileChooserAction::SelectFolder,
            None,
            None,
        );

        chooser.set_local_only(true);

        if chooser.run() == ResponseType::Accept {
            entry.set_text(chooser.filename().unwrap().to_str().unwrap());
        }
    }));

    let output_dir = Entry::new();

    if let Ok(path) = settings.string("paths", "output_dir") {
        output_dir.set_text(&path);
    }

    output_dir.set_icon_from_icon_name(EntryIconPosition::Secondary, Some("document-open"));
    output_dir.set_icon_activatable(EntryIconPosition::Secondary, true);

    output_dir.connect_icon_press(clone!(@strong dialog => move |entry, _, _| {
        let chooser = FileChooserNative::new(
            Some("Select output directory"),
            Some(&dialog),
            FileChooserAction::SelectFolder,
            None,
            None,
        );

        chooser.set_local_only(true);

        if chooser.run() == ResponseType::Accept {
            entry.set_text(chooser.filename().unwrap().to_str().unwrap());
        }
    }));

    let size = SpinButton::new(
        Some(&Adjustment::new(
            settings.double("args", "size").unwrap_or(1000.),
            100.,
            10_000.,
            100.,
            0.,
            0.,
        )),
        5.,
        0,
    );

    let quality = Scale::new(
        Orientation::Horizontal,
        Some(&Adjustment::new(
            settings.double("args", "quality").unwrap_or(90.),
            5.,
            95.,
            1.,
            0.,
            0.,
        )),
    );

    let grid = Grid::new();

    grid.attach(&Label::new(Some("Input directory")), 0, 0, 1, 1);
    grid.attach(&input_dir, 1, 0, 1, 1);

    grid.attach(&Label::new(Some("Output directory")), 0, 1, 1, 1);
    grid.attach(&output_dir, 1, 1, 1, 1);

    grid.attach(&Label::new(Some("Size")), 0, 2, 1, 1);
    grid.attach(&size, 1, 2, 1, 1);

    grid.attach(&Label::new(Some("Quality")), 0, 3, 1, 1);
    grid.attach(&quality, 1, 3, 1, 1);

    grid.set_row_spacing(10);
    grid.set_column_spacing(10);

    dialog.content_area().pack_start(&grid, true, true, 10);

    dialog.show_all();
    application.add_window(&dialog);

    dialog.connect_response(clone!(@strong application => move |dialog, response| {
        dialog.close();

        if response == ResponseType::Ok {
            let input_dir = input_dir.text();
            let output_dir = output_dir.text();
            let size = size.value();
            let quality = quality.value();

            settings.set_string("paths", "input_dir", &input_dir);
            settings.set_string("paths", "output_dir", &output_dir);
            settings.set_double("args", "size", size);
            settings.set_double("args", "quality", quality);

            let _ = settings.save_to_file(&settings_file);

            let mozjpeg_dir = settings.string("paths", "mozjpeg_dir").map(|dir| PathBuf::from(&dir)).unwrap_or_else(|_| {
                let mut mozjpeg_dir = home_dir();

                mozjpeg_dir.push("bin");
                mozjpeg_dir.push("mozjpeg");

                mozjpeg_dir
            });

            show_progress_dialog(&application, mozjpeg_dir, input_dir, output_dir, size, quality);
        }
    }));
}

fn show_progress_dialog(
    application: &Application,
    mozjpeg_dir: PathBuf,
    input_dir: GString,
    output_dir: GString,
    size: f64,
    quality: f64,
) {
    let dialog = Dialog::with_buttons(
        Some("Resize JPEG"),
        NONE_WINDOW,
        DialogFlags::empty(),
        &[("Cancel", ResponseType::Cancel)],
    );

    let progress_bar = ProgressBar::new();

    dialog
        .content_area()
        .pack_start(&progress_bar, true, true, 10);

    dialog.show_all();
    application.add_window(&dialog);

    dialog.connect_response(|dialog, _| {
        dialog.close();
    });

    let (progress_sender, progress_receiver) = MainContext::channel::<Message>(PRIORITY_DEFAULT);

    progress_receiver.attach(
        None,
        clone!(@strong application, @strong dialog => move |message| match message {
            Message::Progress(fraction) => {
                progress_bar.set_fraction(fraction);

                Continue(true)
            },
            Message::Error(message) => {
                dialog.close();

                let dialog = MessageDialog::new(Some(&dialog), DialogFlags::empty(), MessageType::Error, ButtonsType::None, &message);

                dialog.show_all();
                application.add_window(&dialog);

                Continue(false)
            }
            Message::Done => {
                dialog.close();

                Continue(false)
            }
        }),
    );

    spawn(move || {
        progress_sender
            .send(
                match run_operation(
                    &progress_sender,
                    &mozjpeg_dir,
                    Path::new(&input_dir),
                    Path::new(&output_dir),
                    size,
                    quality,
                ) {
                    Ok(()) => Message::Done,
                    Err(err) => Message::Error(err.to_string()),
                },
            )
            .unwrap();
    });
}

enum Message {
    Progress(f64),
    Done,
    Error(String),
}

fn run_operation(
    progress_sender: &Sender<Message>,
    mozjpeg_dir: &Path,
    input_dir: &Path,
    output_dir: &Path,
    size: f64,
    quality: f64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let dir = read_dir(input_dir)?;

    let mut files = Vec::new();

    for entry in dir {
        let entry = entry?;

        if entry.file_type()?.is_file() {
            files.push(entry.file_name());
        }
    }

    if files.is_empty() {
        return Err("Did not find any input files".into());
    }

    let done = AtomicUsize::new(0);

    files.par_iter().try_for_each(|file| {
        let mut input_file = input_dir.to_owned();
        input_file.push(file);

        let mut output_file = output_dir.to_owned();
        output_file.push(file);
        output_file.set_extension("jpg");

        let mut convert = Command::new("convert")
            .arg("-resize")
            .arg(format!("{:.0}x{:.0}", size, size))
            .arg(input_file)
            .arg("TGA:-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let cjpeg = Command::new("./bin/cjpeg")
            .current_dir(mozjpeg_dir)
            .env("LD_LIBRARY_PATH", "./lib64")
            .arg("-quality")
            .arg(format!("{:.0}", quality))
            .arg("-targa")
            .arg("-outfile")
            .arg(output_file)
            .stdin(convert.stdout.take().unwrap())
            .stderr(Stdio::piped())
            .spawn()?;

        let output = convert.wait_with_output()?;

        if !output.status.success() {
            return Err(format!(
                "Failed to resize: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        let output = cjpeg.wait_with_output()?;

        if !output.status.success() {
            return Err(format!(
                "Failed to encode: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        let done = done.fetch_add(1, Ordering::SeqCst) + 1;

        progress_sender
            .send(Message::Progress(done as f64 / files.len() as f64))
            .unwrap();

        Ok(())
    })
}