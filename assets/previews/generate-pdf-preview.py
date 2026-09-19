#!/usr/bin/env python3
"""Regenerate pdf-preview.pdf deterministically, using only Python's stdlib."""
from pathlib import Path


def text(x, y, size, value):
    return f"BT /F1 {size} Tf {x} {y} Td ({value}) Tj ET\n"


def page(heading, width, height, subtitle):
    out = f"0.96 0.97 1 rg 0 0 {width} {height} re f\n"
    out += f"0.12 0.20 0.40 rg 0 {height-150} {width} 150 re f\n"
    out += "1 1 1 rg\n" + text(40, height-70, 32, heading)
    out += text(40, height-110, 16, subtitle)
    out += f"0.1 0.65 0.65 rg 40 {height-255} 140 65 re f\n"
    out += f"0.96 0.58 0.19 rg 205 {height-255} 140 65 re f\n"
    out += f"0.55 0.35 0.85 rg 370 {height-255} 140 65 re f\n"
    out += "0.12 0.20 0.40 rg\n"
    out += text(40, height-305, 20, "Preview verification table")
    for row, (label, value) in enumerate([
        ("Item", "Expected result"),
        ("Text", "Crisp headings and readable labels"),
        ("Colors", "Teal, orange, and violet shapes"),
        ("Navigation", "Two distinct pages"),
    ]):
        y = height - 355 - row * 40
        out += f"{'0.86 0.9 0.96' if row % 2 == 0 else '1 1 1'} rg 40 {y-12} {width-80} 38 re f\n"
        out += "0.12 0.20 0.40 rg\n" + text(50, y, 14, label) + text(190, y, 14, value)
    out += text(40, 35, 12, "Jcode Desktop | supplied PDF payload | offline acceptance fixture")
    return out.encode("ascii")


streams = [page("PDF panel acceptance", 600, 800, "Page 1 / 2 - portrait overview"),
           page("Second PDF page", 800, 600, "Page 2 / 2 - landscape detail")]
objects = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 800 600] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>",
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
] + [f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"endstream" for stream in streams]
pdf = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
offsets = [0]
for number, obj in enumerate(objects, 1):
    offsets.append(len(pdf))
    pdf.extend(f"{number} 0 obj\n".encode() + obj + b"\nendobj\n")
xref = len(pdf)
pdf.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
for offset in offsets[1:]:
    pdf.extend(f"{offset:010} 00000 n \n".encode())
pdf.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
Path(__file__).with_name("pdf-preview.pdf").write_bytes(pdf)
