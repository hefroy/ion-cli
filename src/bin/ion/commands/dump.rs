use anyhow::{Context, Result};
use clap::{Arg, ArgAction, ArgMatches, Command};
use ion_rs::*;
use std::fs::File;
use std::io::{stdin, stdout, StdinLock, Write};

pub fn app() -> Command {
    Command::new("dump")
        .about("Prints Ion in the requested format")
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .default_value("pretty")
                .value_parser(["binary", "text", "pretty", "lines"])
                .help("Output format"),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .short('o')
                .help("Output file [default: STDOUT]"),
        )
        .arg(
            // All argv entries after the program name (argv[0])
            // and any `clap`-managed options are considered input files.
            Arg::new("input")
                .index(1)
                .help("Input file [default: STDIN]")
                .action(ArgAction::Append)
                .trailing_var_arg(true),
        )
}

pub fn run(_command_name: &str, matches: &ArgMatches) -> Result<()> {
    // --format pretty|text|lines|binary
    // `clap` validates the specified format and provides a default otherwise.
    let format = matches
        .get_one::<String>("format")
        .expect("`format` did not have a value");

    // -o filename
    let mut output: Box<dyn Write> = if let Some(output_file) = matches.get_one::<String>("output")
    {
        let file = File::create(output_file).with_context(|| {
            format!(
                "could not open file output file '{}' for writing",
                output_file
            )
        })?;
        Box::new(file)
    } else {
        Box::new(stdout().lock())
    };

    if let Some(input_file_iter) = matches.get_many::<String>("input") {
        for input_file in input_file_iter {
            let file = File::open(input_file)
                .with_context(|| format!("Could not open file '{}'", input_file))?;
            let mut reader = ReaderBuilder::new().build(file)?;
            write_all_in_format(&mut reader, &mut output, format)?;
        }
    } else {
        let input: StdinLock = stdin().lock();
        let mut reader = ReaderBuilder::new().build(input)?;
        write_all_in_format(&mut reader, &mut output, format)?;
    }

    output.flush()?;
    Ok(())
}

/// Constructs the appropriate writer for the given format, then writes all values found in the
/// Reader to the new Writer.
fn write_all_in_format(
    reader: &mut Reader,
    output: &mut Box<dyn Write>,
    format: &str,
) -> IonResult<()> {
    match format {
        "pretty" => {
            let mut writer = TextWriterBuilder::pretty().build(output)?;
            write_all_values(reader, &mut writer)
        }
        "text" => {
            let mut writer = TextWriterBuilder::default().build(output)?;
            write_all_values(reader, &mut writer)
        }
        "lines" => {
            let mut writer = TextWriterBuilder::lines().build(output)?;
            write_all_values(reader, &mut writer)
        }
        "binary" => {
            let mut writer = BinaryWriterBuilder::new().build(output)?;
            write_all_values(reader, &mut writer)
        }
        unrecognized => unreachable!(
            "'format' was '{}' instead of 'pretty', 'text', 'lines', or 'binary'",
            unrecognized
        ),
    }
}

/// Writes each value encountered in the Reader to the provided IonWriter.
fn write_all_values<W: IonWriter>(reader: &mut Reader, writer: &mut W) -> IonResult<()> {
    const FLUSH_EVERY_N: usize = 100;
    let mut values_since_flush: usize = 0;
    let mut annotations = vec![];
    loop {
        match reader.next()? {
            StreamItem::Value(ion_type) | StreamItem::Null(ion_type) => {
                if reader.has_annotations() {
                    annotations.clear();
                    for annotation in reader.annotations() {
                        annotations.push(annotation?);
                    }
                    writer.set_annotations(&annotations);
                }

                if reader.parent_type() == Some(IonType::Struct) {
                    writer.set_field_name(reader.field_name()?);
                }

                if reader.is_null() {
                    writer.write_null(ion_type)?;
                    continue;
                }

                use IonType::*;
                match ion_type {
                    Null => unreachable!("null values are handled prior to this match"),
                    Boolean => writer.write_bool(reader.read_bool()?)?,
                    Integer => writer.write_integer(&reader.read_integer()?)?,
                    Float => {
                        let float64 = reader.read_f64()?;
                        let float32 = float64 as f32;
                        if float32 as f64 == float64 {
                            // No data lost during cast; write it as an f32
                            writer.write_f32(float32)?;
                        } else {
                            writer.write_f64(float64)?;
                        }
                    }
                    Decimal => writer.write_decimal(&reader.read_decimal()?)?,
                    Timestamp => writer.write_timestamp(&reader.read_timestamp()?)?,
                    Symbol => writer.write_symbol(reader.read_symbol()?)?,
                    String => writer.write_string(reader.read_string()?)?,
                    Clob => writer.write_clob(reader.read_clob()?)?,
                    Blob => writer.write_blob(reader.read_blob()?)?,
                    List => {
                        reader.step_in()?;
                        writer.step_in(List)?;
                    }
                    SExpression => {
                        reader.step_in()?;
                        writer.step_in(SExpression)?;
                    }
                    Struct => {
                        reader.step_in()?;
                        writer.step_in(Struct)?;
                    }
                }
            }
            StreamItem::Nothing if reader.depth() > 0 => {
                reader.step_out()?;
                writer.step_out()?;
            }
            StreamItem::Nothing => break,
        }
        if reader.depth() == 0 {
            values_since_flush += 1;
            if values_since_flush == FLUSH_EVERY_N {
                writer.flush()?;
                values_since_flush = 0;
            }
        }
    }
    writer.flush()?;
    Ok(())
}
