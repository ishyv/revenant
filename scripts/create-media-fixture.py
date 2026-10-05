"""Create small native preview fixtures outside the large-file benchmark."""
import argparse
from pathlib import Path
import struct
import wave
import zlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
directory = parser.parse_args().directory.resolve()
directory.mkdir(parents=True, exist_ok=True)

def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00"))
png += chunk(b"IEND", b"")
with (directory / "red.png").open("xb") as output:
    output.write(png)
with (directory / "silence.wav").open("xb") as output:
    with wave.open(output, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(8000)
        audio.writeframes(b"\x00\x00" * 800)
with (directory / "hello.txt").open("x", encoding="utf-8") as output:
    output.write("Native preview acceptance.\n")
print(directory)
