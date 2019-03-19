use std::fs::{self, File};

use std::io;
use std::io::{BufReader, BufWriter};
use std::io::{Read, Write};
use std::io::{Seek, SeekFrom};

use std::mem;

use std::error::{self, Error};
use std::fmt;

use bincode::{self, config};
use crc::{crc32, Hasher32};
use serde::{Deserialize, Serialize};

// 错误统一处理
macro_rules! turn {
    ($expr:expr) => {
        return core::result::Result::Err(core::convert::From::from($expr));
    };
}

#[derive(Debug)]
enum CustomError<'a> {
    IoError(io::Error),
    UnpackError(UnpackError<'a>),
    DeserializeOrSerializeError(Box<bincode::ErrorKind>),
    DecompressError(flate2::DecompressError),
    CompressError(flate2::CompressError),
}

impl<'a> From<io::Error> for CustomError<'a> {
    fn from(err: io::Error) -> CustomError<'a> {
        CustomError::IoError(err)
    }
}

impl<'a> From<UnpackError<'a>> for CustomError<'a> {
    fn from(err: UnpackError<'a>) -> CustomError<'a> {
        CustomError::UnpackError(err)
    }
}

impl<'a> From<Box<bincode::ErrorKind>> for CustomError<'a> {
    fn from(err: Box<bincode::ErrorKind>) -> CustomError<'a> {
        CustomError::DeserializeOrSerializeError(err)
    }
}

impl<'a> From<flate2::DecompressError> for CustomError<'a> {
    fn from(err: flate2::DecompressError) -> CustomError<'a> {
        CustomError::DecompressError(err)
    }
}

impl<'a> From<flate2::CompressError> for CustomError<'a> {
    fn from(err: flate2::CompressError) -> CustomError<'a> {
        CustomError::CompressError(err)
    }
}

#[derive(Debug)]
struct UnpackError<'a> {
    reason: &'a str,
}

impl<'a> fmt::Display for UnpackError<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &self.reason)
    }
}

impl<'a> error::Error for UnpackError<'a> {
    fn description(&self) -> &str {
        &self.reason
    }

    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

// ctce8 的一些文件数据格式定义
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct CTCE8HeaderPart1 {
    flag1: [u32; 4],
    blank_bytes1: [u32; 2],
    flag2: u32,
    blank_bytes2: [u32; 8],
    flag3: u32,
    flag4: [u32; 2],
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct CTCE8HeaderPart2 {
    blank_bytes3: [u32; 13],
    flag5: [u32; 2],
}

#[derive(Serialize, Deserialize, Debug)]
struct CfgHeader {
    flag1: [u32; 2],
    uncompressed_file_size: u32,
    output_data_with_header_size: u32,
    compress_chunk_size: u32,
    compressed_chunk_overlying_crc32: u32,
    cfg_header_crc32: u32,
    blank_bytes: [u32; 8],
}

#[derive(Serialize, Deserialize, Debug)]
struct DataChunkHeader {
    before_compressed_size: u32,
    after_compressed_size: u32,
    chunk_end_offset: u32,
}

const READ_CHUNK_SIZE: usize = 0x10000;
const MIN_ZLIB_COMPRESSED_DATA_SIZE: u32 = 9; // 以 level9 压缩一个字节，长度为9位。 其中 CMF(1) + FLG(1) + ADLER32(4) == 6。参见 https://tools.ietf.org/html/rfc1950

static CTCE8_HEADER_PART1: CTCE8HeaderPart1 = CTCE8HeaderPart1 {
    flag1: [0x99999999, 0x44444444, 0x55555555, 0xAAAAAAAA],
    blank_bytes1: [0; 2],
    flag2: 0x04000000,
    blank_bytes2: [0; 8],
    flag3: 0x40000000,
    flag4: [0x02000000, 0x80000000],
};

static CTCE8_HEADER_PART2: CTCE8HeaderPart2 = CTCE8HeaderPart2 {
    blank_bytes3: [0; 13],
    flag5: [0x04030201, 0],
};

fn pack_to_cfg<'a>(
    xml_file_path: &str,
    ctce8_file_path: &str,
    device_model_string: &str,
) -> Result<(), CustomError<'a>> {
    use flate2::{Compress, Compression, FlushCompress};

    let input_file_size = fs::metadata(&xml_file_path)?.len();

    let input_file = File::open(&xml_file_path)?;
    let mut input_stream = BufReader::new(input_file);

    let output_file = File::create(&ctce8_file_path)?;
    let mut output_stream = BufWriter::new(output_file);

    let mut special_file_size: u32 = 0u32; // 最终输出 ctce8 文件大小 - 128，在 ZXHN F450 上是这样，其他地区机型未知
    let device_model_name_length: u32 = device_model_string.len() as u32;

    let header_placeholder_length = mem::size_of::<CTCE8HeaderPart1>()
        + mem::size_of_val(&special_file_size)
        + mem::size_of::<CTCE8HeaderPart2>()
        + mem::size_of_val(&device_model_name_length)
        + device_model_string.len()
        + mem::size_of::<CfgHeader>();

    {
        let header_placeholder_bytes = vec![0u8; header_placeholder_length];
        output_stream.write_all(&header_placeholder_bytes)?;
    }

    let mut big_endian_config = config();
    big_endian_config.big_endian();
    let mut little_endian_config = config();
    little_endian_config.little_endian();

    let data_chunk_header_size = mem::size_of::<DataChunkHeader>() as u32;

    let mut output_data_size = 0u32; // 文件头之后的数据区大小。数据区的每块数据由块头（data_chunk_header_size == 12 字节）加压缩数据组成
    let mut chunk_end_offset = mem::size_of::<CfgHeader>() as u32; // 注意是以 cfg header 开头作为起始地址，而不是文件初始位置
    let mut compressed_chunk_overlaying_crc32 = 0u32; // 压缩后的数据求 CRC32 值，以前一块压缩数据 CRC32 值为初始值，第一个初始值是 0

    {
        let mut compressed: Vec<u8> =
            Vec::with_capacity(READ_CHUNK_SIZE + MIN_ZLIB_COMPRESSED_DATA_SIZE as usize);
        let mut compressor = Compress::new(Compression::best(), true);

        let mut read_buffer: Vec<u8> = vec![0u8; READ_CHUNK_SIZE];
        let mut read_chunk_size = READ_CHUNK_SIZE;

        let total_read_times: u64 = input_file_size / read_chunk_size as u64 + 1;
        for i in 0..total_read_times {
            if i == total_read_times - 1 {
                read_chunk_size = (input_file_size % read_chunk_size as u64) as usize;
                if read_chunk_size == 0 {
                    break;
                }
            }

            input_stream.read_exact(&mut read_buffer[..read_chunk_size])?;

            compressor.compress_vec(
                &read_buffer[..read_chunk_size],
                &mut compressed,
                FlushCompress::Finish,
            )?;

            if read_chunk_size == READ_CHUNK_SIZE {
                chunk_end_offset += data_chunk_header_size + compressor.total_out() as u32;
            } else {
                chunk_end_offset = 0; // 以偏移值 0 标记接下来的数据块为最后一块
            }

            {
                let data_chunk_header = DataChunkHeader {
                    before_compressed_size: compressor.total_in() as u32,
                    after_compressed_size: compressor.total_out() as u32,
                    chunk_end_offset: chunk_end_offset,
                };

                let encoded: Vec<u8> = big_endian_config.serialize(&data_chunk_header)?;
                output_stream.write_all(&encoded)?;

                output_stream.write_all(&compressed)?;
                output_data_size +=
                    data_chunk_header_size + data_chunk_header.after_compressed_size;

                let mut crc32_digest =
                    crc32::Digest::new_with_initial(crc32::IEEE, compressed_chunk_overlaying_crc32);
                crc32_digest.write(&compressed);
                compressed_chunk_overlaying_crc32 = crc32_digest.sum32();
            }

            compressed.clear();
            compressor.reset();
        }
    }

    output_stream.seek(SeekFrom::Start(0))?;

    {
        let encoded: Vec<u8> = big_endian_config.serialize(&CTCE8_HEADER_PART1)?;
        output_stream.write_all(&encoded)?;
    }

    {
        special_file_size = output_data_size + header_placeholder_length as u32 - 128;
        let encoded: Vec<u8> = little_endian_config.serialize(&special_file_size)?; // 注意这里是用小端写入，也就这个地方特别
        output_stream.write_all(&encoded)?;
    }

    {
        let encoded: Vec<u8> = big_endian_config.serialize(&CTCE8_HEADER_PART2)?;
        output_stream.write_all(&encoded)?;
    }

    {
        let device_model_name_length: u32 = device_model_string.len() as u32;
        let encoded: Vec<u8> = big_endian_config.serialize(&device_model_name_length)?;
        output_stream.write_all(&encoded)?;

        output_stream
            .write_all(&(device_model_string.as_bytes()[..device_model_name_length as usize]))?;
    }

    {
        let mut cfg_header = CfgHeader {
            flag1: [0x01020304, 0],
            uncompressed_file_size: input_file_size as u32,
            output_data_with_header_size: output_data_size + mem::size_of::<CfgHeader>() as u32,
            compress_chunk_size: READ_CHUNK_SIZE as u32,
            compressed_chunk_overlying_crc32: compressed_chunk_overlaying_crc32,
            cfg_header_crc32: 0,
            blank_bytes: [0u32; 8],
        };
        let encoded: Vec<u8> = big_endian_config.serialize(&cfg_header)?;

        let mut crc32_digest = crc32::Digest::new_with_initial(crc32::IEEE, 0u32);
        crc32_digest.write(&(encoded.as_slice()[..24]));
        cfg_header.cfg_header_crc32 = crc32_digest.sum32();

        let encoded: Vec<u8> = big_endian_config.serialize(&cfg_header)?;
        output_stream.write_all(&encoded)?;
    }

    Ok(())
}

fn unpack_to_xml<'a>(ctce8_file_path: &str, xml_file_path: &str) -> Result<(), CustomError<'a>> {
    use flate2::{Decompress, FlushDecompress};

    let input_file_size = fs::metadata(&ctce8_file_path)?.len();

    let input_file = File::open(&ctce8_file_path)?;
    let mut input_stream = BufReader::new(input_file);

    let output_file = File::create(&xml_file_path)?;
    let mut output_stream = BufWriter::new(output_file);

    let mut big_endian_config = config();
    big_endian_config.big_endian();
    let mut little_endian_config = config();
    little_endian_config.little_endian();

    {
        let size = mem::size_of::<CTCE8HeaderPart1>();
        let mut read_buffer: Vec<u8> = vec![0u8; size];
        input_stream.read_exact(&mut read_buffer)?;
        let decoded: CTCE8HeaderPart1 = big_endian_config.deserialize(&read_buffer)?;
        if decoded != CTCE8_HEADER_PART1 {
            turn!(UnpackError {
                reason: "文件格式不正确（CTCE8_HEADER_PART1）"
            })
        }
    }

    let mut special_file_size: u32 = 0;
    {
        let mut read_buffer: Vec<u8> = vec![0u8; mem::size_of_val(&special_file_size)];
        input_stream.read_exact(&mut read_buffer)?;
        special_file_size = little_endian_config.deserialize(&read_buffer)?;
    }

    if input_file_size - special_file_size as u64 != 128 {
        turn!(UnpackError {
            reason: "文件大小不正确，可能不兼容该版本 cfg 文件"
        })
    }

    {
        let size = mem::size_of::<CTCE8HeaderPart2>();
        let mut read_buffer: Vec<u8> = vec![0u8; size];
        input_stream.read_exact(&mut read_buffer)?;
        let decoded: CTCE8HeaderPart2 = big_endian_config.deserialize(&read_buffer)?;
        if decoded != CTCE8_HEADER_PART2 {
            turn!(UnpackError {
                reason: "文件格式不正确（CTCE8_HEADER_PART2）"
            })
        }
    }

    {
        let mut device_model_string_length: u32 = 0;
        let mut read_buffer: Vec<u8> = vec![0u8; mem::size_of_val(&device_model_string_length)];
        input_stream.read_exact(&mut read_buffer)?;
        device_model_string_length = big_endian_config.deserialize(&read_buffer)?;

        let placeholder_length = mem::size_of::<CTCE8HeaderPart1>()
            + mem::size_of_val(&special_file_size)
            + mem::size_of::<CTCE8HeaderPart2>()
            + mem::size_of_val(&device_model_string_length)
            + device_model_string_length as usize
            + mem::size_of::<CfgHeader>();
        if MIN_ZLIB_COMPRESSED_DATA_SIZE as u64 > input_file_size - placeholder_length as u64 {
            turn!(UnpackError {
                reason: "文件数据不正确，压缩的数据丢失"
            })
        }

        input_stream.seek(SeekFrom::Current(device_model_string_length as i64))?;
    }

    let cfg_header: CfgHeader;
    {
        let mut read_buffer: Vec<u8> = vec![0u8; mem::size_of::<CfgHeader>()];
        input_stream.read_exact(&mut read_buffer)?;
        cfg_header = big_endian_config.deserialize(&read_buffer)?;

        if cfg_header.flag1 != [0x01020304u32, 0u32] || cfg_header.blank_bytes != [0u32; 8] {
            turn!(UnpackError {
                reason: "文件格式不正确，cfc header magic位 校验失败"
            })
        }

        let mut crc32_digest = crc32::Digest::new_with_initial(crc32::IEEE, 0u32);
        crc32_digest.write(&(read_buffer.as_slice()[..24]));
        if cfg_header.cfg_header_crc32 != crc32_digest.sum32() {
            turn!(UnpackError {
                reason: "文件格式不正确，cfg header crc32 校验失败"
            })
        }
    }

    let mut decompressor = Decompress::new(true);

    let cfg_header_size = mem::size_of::<CfgHeader>();
    let data_chunk_header_size = mem::size_of::<DataChunkHeader>() as u32;
    let mut last_chunk_end_offset = cfg_header_size as u32;
    let mut compressed_chunk_overlaying_crc32 = 0u32;
    loop {
        let data_chunk_header: DataChunkHeader;
        {
            let mut read_buffer: Vec<u8> = vec![0u8; data_chunk_header_size as usize];
            input_stream.read_exact(&mut read_buffer)?;
            data_chunk_header = big_endian_config.deserialize(&read_buffer)?;
        }

        {
            let mut read_buffer: Vec<u8>;
            if data_chunk_header.chunk_end_offset != 0 {
                read_buffer = vec![
                    0u8;
                    (data_chunk_header.chunk_end_offset
                        - data_chunk_header_size
                        - last_chunk_end_offset) as usize
                ];
                input_stream.read_exact(&mut read_buffer)?;
                last_chunk_end_offset = data_chunk_header.chunk_end_offset;
            } else {
                read_buffer = Vec::with_capacity(data_chunk_header.after_compressed_size as usize);
                input_stream.read_to_end(&mut read_buffer)?;
            }

            let mut write_buffer: Vec<u8> =
                Vec::with_capacity(data_chunk_header.before_compressed_size as usize);
            decompressor.decompress_vec(
                &read_buffer,
                &mut write_buffer,
                FlushDecompress::Finish,
            )?;

            output_stream.write_all(&write_buffer)?;
            decompressor.reset(true);

            let mut crc32_digest =
                crc32::Digest::new_with_initial(crc32::IEEE, compressed_chunk_overlaying_crc32);
            crc32_digest.write(&read_buffer);
            compressed_chunk_overlaying_crc32 = crc32_digest.sum32();

            if data_chunk_header.chunk_end_offset == 0 {
                break;
            }
        }
    }

    if compressed_chunk_overlaying_crc32 != cfg_header.compressed_chunk_overlying_crc32 {
        turn!(UnpackError {
            reason: "文件数据不正确，压缩数据 CRC32 校验失败"
        })
    }

    Ok(())
}

fn main() {
    use clap::{App, Arg, SubCommand};

    let matches = App::new("CTCE8 file pack/unpack tool")
        .version("0.1.0")
        .subcommand(
            SubCommand::with_name("pack")
                .about("Package the XML file into a CTCE8 CFG file")
                .arg(
                    Arg::with_name("INPUT")
                        .help("Sets the XML file path")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::with_name("OUTPUT")
                        .help("Sets the CFG file path")
                        .required(true)
                        .index(2),
                )
                .arg(
                    Arg::with_name("MODEL")
                        .help("Sets the device model string")
                        .required(true)
                        .index(3),
                ),
        )
        .subcommand(
            SubCommand::with_name("unpack")
                .about("Unpack the CTCE8 CFG file into an XML file")
                .arg(
                    Arg::with_name("INPUT")
                        .help("Sets the CFG file path")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::with_name("OUTPUT")
                        .help("Sets the XML file path")
                        .required(true)
                        .index(2),
                ),
        )
        .get_matches();

    if let Some(matches) = matches.subcommand_matches("pack") {
        if let Err(err) = pack_to_cfg(
            matches.value_of("INPUT").unwrap(),
            matches.value_of("OUTPUT").unwrap(),
            matches.value_of("MODEL").unwrap(),
        ) {
            println!("{:?}", err);
        }

        return;
    }

    if let Some(matches) = matches.subcommand_matches("unpack") {
        if let Err(err) = unpack_to_xml(
            matches.value_of("INPUT").unwrap(),
            matches.value_of("OUTPUT").unwrap(),
        ) {
            println!("{:?}", err);
        }

        return;
    }

    println!("{}", matches.usage());
    println!("");
    println!(
        "Try `{} --help' for more information.",
        std::env::current_exe()
            .unwrap_or(std::path::Path::new("ctce8_cfg_tool").to_path_buf())
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
    );
}
