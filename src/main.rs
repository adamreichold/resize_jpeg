use std::error::Error;
use std::fs::{create_dir_all, read_dir, write};
use std::panic::catch_unwind;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::spawn;

use gio::prelude::{ApplicationExt, ApplicationExtManual};
use glib::{
    clone, user_config_dir, Continue, GString, KeyFile, KeyFileFlags, MainContext, Sender,
    PRIORITY_DEFAULT,
};
use gtk::{
    prelude::{
        BoxExt, DialogExt, EntryExt, FileChooserExt, GridExt, GtkApplicationExt, GtkWindowExt,
        NativeDialogExt, ProgressBarExt, RangeExt, ScaleExt, SpinButtonExt, WidgetExt,
    },
    Adjustment, Application, ButtonsType, Dialog, DialogFlags, Entry, EntryIconPosition,
    FileChooserAction, FileChooserNative, Grid, Label, MessageDialog, MessageType, Orientation,
    ProgressBar, ResponseType, Scale, SpinButton, Window,
};
use image::{imageops::FilterType, io::Reader as ImageReader};
use mozjpeg::{ColorSpace, Compress, ScanMode};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rexiv2::Metadata;

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
        Window::NONE,
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

        chooser.set_current_folder(entry.text().as_str());
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

        chooser.set_current_folder(entry.text().as_str());
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

    quality.set_digits(0);

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

            show_progress_dialog(&application, input_dir, output_dir, size, quality);
        }
    }));
}

fn show_progress_dialog(
    application: &Application,
    input_dir: GString,
    output_dir: GString,
    size: f64,
    quality: f64,
) {
    let dialog = Dialog::with_buttons(
        Some("Resize JPEG"),
        Window::NONE,
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

                let dialog = MessageDialog::new(Some(&dialog), DialogFlags::empty(), MessageType::Error, ButtonsType::Close, &message);

                dialog.connect_response(|dialog, _| {
                    dialog.close();
                });

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

    create_dir_all(output_dir)?;

    let done = AtomicUsize::new(0);

    files.par_iter().try_for_each(|file| {
        let mut input_file = input_dir.to_owned();
        input_file.push(file);

        let mut output_file = output_dir.to_owned();
        output_file.push(file);
        output_file.set_extension("jpg");

        let image = ImageReader::open(&input_file)?
            .decode()?
            .resize(size as u32, size as u32, FilterType::Lanczos3)
            .into_rgb8();

        let metadata = Metadata::new_from_path(&input_file)?;

        let buffer = catch_unwind(|| {
            let mut compress = Compress::new(ColorSpace::JCS_RGB);
            compress.set_size(image.width() as usize, image.height() as usize);
            compress.set_mem_dest();

            compress.set_scan_optimization_mode(ScanMode::AllComponentsTogether);
            compress.set_use_scans_in_trellis(true);
            compress.set_quality(quality as f32);

            compress.start_compress();
            assert!(compress.write_scanlines(&image));
            compress.finish_compress();

            compress.data_to_vec().unwrap()
        })
        .map_err(|_| "Failed to compress JPEG")?;

        write(&output_file, buffer)?;

        metadata.save_to_file(&output_file)?;

        let done = done.fetch_add(1, Ordering::SeqCst) + 1;

        progress_sender
            .send(Message::Progress(done as f64 / files.len() as f64))
            .unwrap();

        Ok(())
    })
}
