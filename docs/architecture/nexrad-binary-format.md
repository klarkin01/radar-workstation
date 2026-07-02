# NEXRAD Level II Binary Format Reference

This document describes the binary layout of every data structure in the NEXRAD
Level II chunk stream, as empirically confirmed against KDOX sample data using the
inspection utilities in `utility/nexrad-inspect/`. Values marked **confirmed** were
directly read from the binary and cross-validated with the Python decoder. All byte
offsets are zero-indexed. Multi-byte integers are big-endian unless noted.

Reference ICD documents (available from the ROC at
`https://www.roc.noaa.gov/wsr88d/BuildInfo/Files.aspx`):
- **ICD 2620002** — RDA/RPG Interface Control (message types, Message 31 layout)
- **ICD 2620010** — Archive II Level II File Format (volume header, chunk structure)

---

## 1. Chunk File Detection

Three chunk types are encountered in the real-time stream from the
`unidata-nexrad-level2-chunks` S3 bucket. They are identified by the first 8 bytes
of the raw file before any decompression.

| Chunk type      | Suffix | Detection rule                                    |
|-----------------|--------|---------------------------------------------------|
| Start chunk     | `-S`   | `bytes[0..4] == b"AR2V"` OR `bytes[28..32] == b"BZh9"` |
| Intermediate    | `-I`   | `bytes[4..8] == b"BZh9"` (unsigned length prefix) |
| End chunk       | `-E`   | `bytes[4..8] == b"BZh9"` AND `i32::from_be_bytes(bytes[0..4]) < 0` |

The `-E` detection must be tested before `-I` since both have `BZh9` at offset 4.
Read the first 4 bytes as a signed `i32`; a negative value indicates end-of-volume.

---

## 2. Chunk File Outer Envelopes

### 2.1 Start Chunk (`-S`)

Contains volume-level metadata: VCP definition (Message 5), RDA status (Message 2),
performance data (Message 3), clutter filter map (Message 15), and adaptation data
(Message 18). Contains **no radial data** (no Message 31).

```
[24 bytes]          Volume Header (see Section 3)
[4 bytes]           u32 BE — compressed block length
[N bytes]           BZ2-compressed message stream
[4 bytes]           u32 BE — next block length (if any)
[N bytes]           BZ2-compressed message stream
  ...
[4 bytes]           0xFFFFFFFF — sentinel, end of compressed records
```

After decompression, each block yields a flat NEXRAD message stream (no header to
skip; the volume header precedes the first block and is not repeated).

### 2.2 Intermediate Chunk (`-I`)

Contains 120 Message 31 radials covering approximately 60° of azimuthal sweep.

```
[4 bytes]           u32 BE — compressed block length
[N bytes]           BZ2-compressed message stream
[4 bytes]           u32 BE — next block length (if any; may reach EOF without sentinel)
  ...
```

There is **no volume header**. The decompressed output begins immediately with the
first CTM header. The stream should be treated as starting at byte 0.

### 2.3 End Chunk (`-E`)

Contains the final 120 Message 31 radials of the volume. Always a single BZ2 block.

```
[4 bytes]           i32 BE (negative) — abs(value) = compressed block length
[N bytes]           BZ2-compressed message stream
```

The negative sign signals end-of-volume to consumers. `abs(length)` is the byte
count of the BZ2 data. As with `-I`, the decompressed output begins at byte 0
with the first CTM header.

---

## 3. Volume Header (24 bytes)

Present at byte 0 of the raw `-S` file and within the decompressed `-S` stream. Not
present in `-I` or `-E` decompressed output. **Confirmed from KDOX sample.**

| Offset | Size | Type          | Field              | Example (KDOX)    |
|--------|------|---------------|--------------------|-------------------|
| 0      | 9    | ASCII         | `file_type`        | `"AR2V0006."`     |
| 9      | 3    | ASCII         | `extension`        | `"145"`           |
| 12     | 4    | `u32`         | `julian_date`      | `20634`           |
| 16     | 4    | `u32`         | `time_ms`          | `64860005`        |
| 20     | 4    | ASCII         | `icao`             | `"KDOX"`          |

`julian_date`: days since 1970-01-01, where day 1 = 1970-01-01. Converts to a
`chrono::NaiveDate` as `NaiveDate::from_ymd(1970,1,1) + Duration::days(julian_date - 1)`.

`time_ms`: milliseconds since midnight UTC.

---

## 4. Decompressed Message Stream

After decompression and volume-header handling, the message stream is a flat
sequence of NEXRAD message records. **Consumers should strip the 24-byte volume
header before passing the stream to the decoder.**

Each record:
```
[12 bytes]          CTM Header (opaque legacy framing, not parsed)
[16 bytes]          Message Header (see Section 5)
[variable]          Message body (see Section 6 for type 31)
```

Advancing through the stream:
- **Message 31 (variable length):** advance by `12 + (size_hw × 2)`, rounded up to
  the next 4-byte boundary.
- **All other message types (fixed legacy size):** advance by **2432 bytes**.

When `size_hw == 0`, the record is padding; advance by 2432 bytes and continue.

---

## 5. Message Header (16 bytes, at record offset 12)

| Offset | Size | Type  | Field           | Notes                                    |
|--------|------|-------|-----------------|------------------------------------------|
| 0      | 2    | `u16` | `size_hw`       | Message size in halfwords (2-byte units) |
| 2      | 1    | `u8`  | `rda_channel`   | RDA channel identifier                   |
| 3      | 1    | `u8`  | `msg_type`      | Message type (31 = radial data)          |
| 4      | 2    | `u16` | `seq_num`       | Sequence number                          |
| 6      | 2    | `u16` | `julian_date`   | NEXRAD Julian date (same epoch as §3)    |
| 8      | 4    | `u32` | `time_ms`       | Milliseconds since midnight UTC          |
| 12     | 2    | `u16` | `num_segments`  | Number of segments (usually 1)           |
| 14     | 2    | `u16` | `segment_num`   | Segment number (usually 1)               |

`size_hw × 2` = byte count of the message header (16 bytes) + message body combined.
The body alone is `(size_hw × 2) - 16` bytes.

### Known Message Types

| Type | Description                          | Found in     |
|------|--------------------------------------|--------------|
| 2    | RDA Status Data                      | `-S` chunk   |
| 3    | Performance/Maintenance Data         | `-S` chunk   |
| 5    | Volume Coverage Pattern (VCP)        | `-S` chunk   |
| 15   | Clutter Filter Map                   | `-S` chunk   |
| 18   | RDA Adaptation Data                  | `-S` chunk   |
| 31   | Digital Radar Data (radial)          | `-I`, `-E`   |

---

## 6. Message 31 Body Layout

The Message 31 body begins at **record offset 28** (12-byte CTM + 16-byte message
header). All block pointers within Message 31 are offsets from **byte 0 of the
body** (i.e., relative to record offset 28).

### 6.1 Fixed Header (32 bytes, body offset 0)

**Confirmed from KDOX VCP 35 sample.**

| Body offset | Size | Type       | Field                  | Notes                                            |
|-------------|------|------------|------------------------|--------------------------------------------------|
| 0           | 4    | `[u8; 4]`  | `radar_id`             | ICAO site ID (ASCII, e.g. `"KDOX"`)             |
| 4           | 4    | `u32`      | `ms_since_midnight`    | Radial timestamp: ms since midnight UTC          |
| 8           | 2    | `u16`      | `julian_date`          | Radial date (NEXRAD Julian, same epoch as §3)    |
| 10          | 2    | `u16`      | `az_num`               | Azimuth number within elevation (1-indexed)      |
| 12          | 4    | `f32`      | `az_angle`             | Azimuth angle in degrees (0–360)                 |
| 16          | 1    | `u8`       | `compression`          | 0 = uncompressed (always 0 in practice)          |
| 17          | 1    | `u8`       | `spare`                | Reserved                                         |
| 18          | 2    | `u16`      | `radial_len`           | Radial length in halfwords                       |
| 20          | 1    | `u8`       | `az_spacing`           | 1 = 1.0° spacing, 2 = 0.5° (super-resolution)   |
| 21          | 1    | `u8`       | `radial_status`        | See Radial Status table below                    |
| 22          | 1    | `u8`       | `el_num`               | Elevation number within volume (1-indexed)       |
| 23          | 1    | `u8`       | `sector_cut_num`       | Sector cut number                                |
| 24          | 4    | `f32`      | `el_angle`             | Elevation angle in degrees                       |
| 28          | 1    | `u8`       | `radial_spot_blanking` | Spot blanking status bitmask                     |
| 29          | 1    | `u8`       | `az_index_mode`        | Azimuth indexing mode                            |
| 30          | 2    | `u16`      | `num_data_blks`        | Total data block count (vol + el + rad + moments)|

#### Radial Status Codes (`radial_status`)

| Code | Meaning                             | Notes                                          |
|------|-------------------------------------|------------------------------------------------|
| 0    | Start of Elevation                  | First radial of a new tilt                     |
| 1    | Intermediate                        | Mid-elevation radial                           |
| 2    | End of Elevation                    | Last radial of the tilt; `complete = true`     |
| 3    | Start of Volume                     | First radial of the volume (implies tilt 1)    |
| 4    | End of Volume                       | Last radial; signals volume completion         |
| 5    | Start of Elevation (SAILS)          | SAILS supplemental low-level cut               |

Code 3 (Start of Volume) is the only radial that carries a populated RVOL block
with site metadata (lat/lon, VCP number). All other radials carry a null or absent
RVOL pointer.

### 6.2 Block Pointer Table (body offset 32)

Immediately follows the fixed header. Each pointer is a `u32` body-relative offset
to the start of that data block's preamble.

| Slot         | Size | Type  | Field        | Notes                                        |
|--------------|------|-------|--------------|----------------------------------------------|
| 32           | 4    | `u32` | `vol_ptr`    | Offset to RVOL block (0 if absent)           |
| 36           | 4    | `u32` | `el_ptr`     | Offset to RELV block (0 if absent)           |
| 40           | 4    | `u32` | `rad_ptr`    | Offset to RRAD block (0 if absent)           |
| 44           | 4    | `u32` | `moment_ptr[0]` | Offset to first moment block              |
| 48           | 4    | `u32` | `moment_ptr[1]` | ...                                       |
| ...          | ...  | ...   | ...          | Up to 9 moment pointers (9 defined by ICD)  |

Number of moment pointers = `num_data_blks - 3` (subtracting RVOL, RELV, RRAD).
The pointer table occupies `num_data_blks × 4` bytes starting at body offset 32.

**Example (KDOX VCP 35 Start of Volume, `num_data_blks = 8`, 5 moment ptrs):**

| Body offset | Value | Points to        |
|-------------|-------|------------------|
| 32          | 72    | RVOL block       |
| 36          | 124   | RELV block       |
| 40          | 136   | RRAD block       |
| 44          | 164   | DREF block       |
| 48          | 2024  | DZDR block       |
| 52          | 4436  | DPHI block       |
| 56          | 6848  | DRHO block       |
| 60          | 8068  | DCFP block       |

---

## 7. Data Block Preambles

Two preamble sizes are used, depending on block type:

**Constant blocks** (RVOL, RELV, RRAD): 6-byte preamble.

| Preamble offset | Size | Type      | Field        |
|-----------------|------|-----------|--------------|
| 0               | 4    | `[u8; 4]` | `block_id`   |
| 4               | 2    | `u16`     | `block_size` |

**Moment data blocks** (DREF, DVEL, DSW, DZDR, DPHI, DRHO, DCFP): 8-byte preamble.

| Preamble offset | Size | Type      | Field        | Notes                            |
|-----------------|------|-----------|--------------|----------------------------------|
| 0               | 4    | `[u8; 4]` | `block_id`   |                                  |
| 4               | 2    | `u16`     | `block_size` | Observed as 0 in all sample data |
| 6               | 2    | `u16`     | `version`    | Observed as 0 in all sample data |

For moment blocks, `block_size` is 0 in all observed data. Block navigation must
use the explicit moment pointers from §6.2, not this field.

---

## 8. RVOL — Volume Constants Block

Located at `body[vol_ptr]`. Present only on radials with status 3 (Start of Volume);
pointer is 0 on all other radials. **Confirmed from KDOX VCP 35.**

| Block offset | Size | Type      | Field              | Example (KDOX)    |
|--------------|------|-----------|--------------------|-------------------|
| 0            | 4    | `[u8; 4]` | `block_id`         | `"RVOL"`          |
| 4            | 2    | `u16`     | `block_size`       | 52                |
| 6            | 1    | `u8`      | `major`            | 3                 |
| 7            | 1    | `u8`      | `minor`            | 0                 |
| 8            | 4    | `f32`     | `lat`              | 38.8258°          |
| 12           | 4    | `f32`     | `lon`              | −75.4401°         |
| 16           | 2    | `i16`     | `site_amsl`        | 15 m              |
| 18           | 2    | `u16`     | `feedhorn_agl`     | 34 m              |
| 20           | 4    | `f32`     | `calib_dbz`        | −43.3942          |
| 24           | 4    | `f32`     | `txpower_h`        | 191.9358 kW       |
| 28           | 4    | `f32`     | `txpower_v`        | 178.4238 kW       |
| 32           | 4    | `f32`     | `sys_zdr`          | 0.4906 dB         |
| 36           | 4    | `f32`     | `phidp0`           | 60.0°             |
| 40           | 2    | `u16`     | `vcp`              | 35                |
| 42           | 2    | `u16`     | `processing_status`| 0x0003            |
| 44           | 8    | `[u8; 8]` | spare              | all zeros         |

`block_size` = 52 = 44 bytes of defined fields + 8 bytes spare. Parsers must
advance to the next block using `el_ptr`, not by assuming block size.

---

## 9. RELV — Elevation Constants Block

Located at `body[el_ptr]`. **Confirmed from KDOX VCP 35.**

| Block offset | Size | Type      | Field          | Notes              |
|--------------|------|-----------|----------------|--------------------|
| 0            | 4    | `[u8; 4]` | `block_id`     | `"RELV"`           |
| 4            | 2    | `u16`     | `block_size`   | 12                 |
| 6            | 4    | `f32`     | `atmos_atten`  | dB/km              |
| 10           | 2    | `u16`     | `calib_const`  |                    |

Total block = 12 bytes.

---

## 10. RRAD — Radial Constants Block

Located at `body[rad_ptr]`. Two versions exist, distinguished by `block_size`.
**Confirmed from KDOX VCP 35 (v1 format).**

### 10.1 RRAD v1 (`block_size == 28`)

| Block offset | Size | Type      | Field           | Notes                                       |
|--------------|------|-----------|-----------------|---------------------------------------------|
| 0            | 4    | `[u8; 4]` | `block_id`      | `"RRAD"`                                    |
| 4            | 2    | `u16`     | `block_size`    | 28                                          |
| 6            | 2    | `u16`     | `unamb_range`   | In 1/8 km units; divide by 8.0 to get km   |
| 8            | 4    | `f32`     | `horiz_noise`   | dBm                                         |
| 12           | 4    | `f32`     | `vert_noise`    | dBm                                         |
| 16           | 2    | `u16`     | `nyquist_vel`   | In 0.01 m/s units; divide by 100.0 to get m/s |
| 18           | 2    | `u16`     | spare           |                                             |
| 20           | 4    | `f32`     | `calib_const_h` |                                             |
| 24           | 4    | `f32`     | `calib_const_v` |                                             |

Total block = 28 bytes (6-byte preamble + 22 bytes data).

**Example (KDOX VCP 35):** `unamb_range` = 4670 raw → 583.75 km.
`nyquist_vel` = 837 raw → 8.37 m/s.

### 10.2 RRAD v2 (`block_size == 32`)

Identical to v1, with two extra fields inserted after `spare`:

| Block offset | Size | Type      | Field           |
|--------------|------|-----------|-----------------|
| 20           | 2    | `u16`     | `radial_flags`  |
| 22           | 2    | `u16`     | spare2          |
| 24           | 4    | `f32`     | `calib_const_h` |
| 28           | 4    | `f32`     | `calib_const_v` |

Total block = 32 bytes (6-byte preamble + 26 bytes data).

**Detection rule:** `if block_size >= 32 { v2 } else { v1 }`.

---

## 11. Moment Data Blocks

Each moment block is pointed to by a slot in the pointer table. Blocks may appear
in any order; use the pointer, not positional assumptions.

### 11.1 Moment Block Header (fields at preamble offset 8–28)

The 8-byte preamble is described in §7. The moment data header follows immediately.

| Block offset | Size | Type      | Field            | Notes                                          |
|--------------|------|-----------|------------------|------------------------------------------------|
| 0            | 4    | `[u8; 4]` | `block_id`       | See block type table below                     |
| 4            | 2    | `u16`     | `block_size`     | 0 in observed data; do not use for navigation  |
| 6            | 2    | `u16`     | `version`        | 0 in observed data                             |
| 8            | 2    | `u16`     | `gate_count`     | Number of gates in this radial                 |
| 10           | 2    | `u16`     | `first_gate`     | Range to center of first gate, **in meters**   |
| 12           | 2    | `u16`     | `gate_width`     | Gate spacing, **in meters**                    |
| 14           | 2    | `u16`     | `tover`          | Threshold over (SNR application parameter)     |
| 16           | 2    | `u16`     | `snr_threshold`  | SNR threshold (×8 = dB)                        |
| 18           | 1    | `u8`      | spare            |                                                |
| 19           | 1    | `u8`      | `word_size`      | Bits per gate: 8 or 16                         |
| 20           | 4    | `f32`     | `scale`          | Physical value scaling factor                  |
| 24           | 4    | `f32`     | `offset`         | Physical value offset                          |
| 28           | var  | —         | gate data        | `gate_count × (word_size / 8)` bytes           |

`first_gate` and `gate_width` are in meters. Divide by 1000.0 to get km.

### 11.2 Moment Block Types

| `block_id` | Moment                        | Typical word_size |
|------------|-------------------------------|-------------------|
| `"DREF"`   | Reflectivity (Z)              | 8 bits            |
| `"DVEL"`   | Radial Velocity (V)           | 8 bits            |
| `"DSW "`   | Spectrum Width (W)            | 8 bits            |
| `"DZDR"`   | Differential Reflectivity (ZDR) | 16 bits         |
| `"DPHI"`   | Differential Phase (KDP/PhiDP) | 16 bits          |
| `"DRHO"`   | Correlation Coefficient (CC)  | 8 bits            |
| `"DCFP"`   | Clutter Filter Power (CFP)    | 8 bits            |

Note: `"DSW "` has a trailing space in the binary (the block_id is 4 bytes, always).

### 11.3 Gate Data Layout

Gate data begins at block offset 28. Each gate is `word_size / 8` bytes, big-endian
for 16-bit values.

**Reserved raw values (do not convert to physical):**

| Raw value | Meaning                           |
|-----------|-----------------------------------|
| 0         | Below SNR threshold (no data)     |
| 1         | Range folded (ambiguous range)    |

All other raw values convert to physical units via:
```
physical = (raw_u8_or_u16 as f32 - offset) / scale
```

### 11.4 Confirmed Scale/Offset Values (KDOX VCP 35)

| Block  | word_size | scale     | offset   | gate_count | first_gate | gate_width |
|--------|-----------|-----------|----------|------------|------------|------------|
| DREF   | 8 bit     | 2.0       | 66.0     | 1832       | 2125 m     | 250 m      |
| DZDR   | 16 bit    | 32.0      | 418.0    | 1192       | 2125 m     | 250 m      |
| DPHI   | 16 bit    | 2.8361    | 2.0      | 1192       | 2125 m     | 250 m      |
| DRHO   | 8 bit     | 300.0     | −60.5    | 1192       | 2125 m     | 250 m      |
| DCFP   | 8 bit     | 1.0       | 8.0      | 1832       | 2125 m     | 250 m      |
| DVEL   | 8 bit     | 2.0       | 129.0    | 688        | 2125 m     | 250 m      |
| DSW    | 8 bit     | 2.0       | 129.0    | 688        | 2125 m     | 250 m      |

DVEL and DSW appear only on higher tilts in VCP 35 (Doppler-only tilt strategy).
DZDR, DPHI, DRHO appear only on tilts where dual-pol data is collected.

---

## 12. Physical Unit Conversions

| Field           | Raw unit          | To physical                  | Example                  |
|-----------------|-------------------|------------------------------|--------------------------|
| `unamb_range`   | 1/8 km            | `raw / 8.0`                  | 4670 → 583.75 km         |
| `nyquist_vel`   | 0.01 m/s          | `raw / 100.0`                | 837 → 8.37 m/s           |
| `first_gate`    | meters            | `raw / 1000.0`               | 2125 → 2.125 km          |
| `gate_width`    | meters            | `raw / 1000.0`               | 250 → 0.250 km           |
| Gate data (Z)   | raw u8            | `(raw - 66.0) / 2.0`         | 133 → 33.5 dBZ           |
| Gate data (V)   | raw u8            | `(raw - 129.0) / 2.0`        | 129 → 0.0 m/s            |
| Gate data (RHO) | raw u8            | `(raw + 60.5) / 300.0`       | 241 → 1.005 (clamp to 1) |
| Gate data (ZDR) | raw u16           | `(raw - 418.0) / 32.0`       | 450 → 1.0 dB             |
| Gate data (PHI) | raw u16           | `(raw - 2.0) / 2.8361`       |                          |

General formula: `physical = (raw as f32 - offset) / scale`.

---

## 13. Memory Layout Map (Start-of-Volume Radial, KDOX VCP 35)

This shows how a complete start-of-volume Message 31 record is laid out in memory.
Offsets are from the start of the raw record (CTM header at byte 0).

```
[  0..  12]  CTM Header (12 bytes, opaque)
[ 12..  28]  Message Header (16 bytes)
                 size_hw=4972 → message (header+body) = 9944 bytes
                 msg_type=31
[ 28..  60]  Msg31 Fixed Header (32 bytes)  ← body[0..32]
                 radar_id="KDOX"  az=226.25°  el=0.39°
                 radial_status=3 (StartOfVolume)
                 el_num=1  num_data_blks=8
[ 60..  92]  Block Pointer Table (8 × 4 = 32 bytes)  ← body[32..64]
                 vol_ptr=72  el_ptr=124  rad_ptr=136
                 moment_ptrs=[164, 2024, 4436, 6848, 8068]
[ 92.. 100]  Gap (8 bytes)  ← body[64..72]
             Note: all pointers below are body-relative offsets
[100.. 152]  RVOL block (52 bytes)  ← body[72..124]
[152.. 164]  RELV block (12 bytes)  ← body[124..136]
[164.. 192]  RRAD block (28 bytes)  ← body[136..164]
[192.. 220]  DREF preamble + header (28 bytes)  ← body[164..192]
[220..2052]  DREF gate data (1832 × 1 byte = 1832 bytes)  ← body[192..2024]
[2052..2080] DZDR preamble + header (28 bytes)  ← body[2024..2052]
[2080..4464] DZDR gate data (1192 × 2 bytes = 2384 bytes)  ← body[2052..4436]
[4464..4492] DPHI preamble + header (28 bytes)  ← body[4436..4464]
[4492..6876] DPHI gate data (1192 × 2 bytes = 2384 bytes)  ← body[4464..6848]
[6876..6904] DRHO preamble + header (28 bytes)  ← body[6848..6876]
[6904..8096] DRHO gate data (1192 × 1 byte = 1192 bytes)  ← body[6876..8068]
[8096..8124] DCFP preamble + header (28 bytes)  ← body[8068..8096]
[8124..9956] DCFP gate data (1832 × 1 byte = 1832 bytes)  ← body[8096..9928]
```

Total record size = 9956 bytes (12 CTM + 9944 message, already 4-byte aligned).

---

## 14. Known Discrepancies from CLAUDE.md

The `CLAUDE.md` notes in the repository root were written from ICD reading before
empirical validation. The following entries were superseded by binary inspection:

| Field                    | CLAUDE.md says | Empirically confirmed |
|--------------------------|----------------|-----------------------|
| RVOL `block_size`        | 44             | **52** (8 spare bytes at end) |
| RRAD v1 `block_size`     | 20             | **28** (total including preamble) |
| RRAD v2 `block_size`     | 28             | **32** (inferred from struct; v2 not yet seen in KDOX data) |
| Moment `block_size`      | (not noted)    | **0** in all observed data |
| VCP in KDOX sample       | 212            | **35** (clear-air surveillance) |

`CLAUDE.md` was updated with the confirmed VCP=35 values. The RRAD block_size
discrepancy may arise from the ICD specifying data-only size (22 bytes for v1)
versus the binary storing total block size (28 bytes); the source of the "20" value
is unclear.
