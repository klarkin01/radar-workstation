"""
nexrad_msg31.py — NEXRAD Message 31 decoder
Radar Workstation, Meteorological — development utility

Parses the binary structure of a Message 31 radial per ICD 2620002.
Imported by inspect_messages.py and inspect_geometry.py.

Message 31 structure (after the 12-byte CTM + 16-byte message header):
  [Msg31 Data Header — 68 bytes]
  [Volume Data Constant block — if present]
  [Elevation Data Constant block — if present]
  [Radial Data Constant block — if present]
  [Per-moment data blocks — variable, one per moment present]
"""

import struct


# ---------------------------------------------------------------------------
# ICD constants
# ---------------------------------------------------------------------------

CTM_HEADER_SIZE = 12
MSG_HEADER_SIZE = 16   # size_hw(2) + channel(1) + type(1) + seq(2) + date(2) + time(4) + segs(2) + seg(2)
MSG31_HDR_OFFSET = CTM_HEADER_SIZE + MSG_HEADER_SIZE  # = 28

# Message 31 Data Header (68 bytes, ICD Table V)
# radar_id(4s), ms_since_midnight(L), julian_date(H), az_num(H), az_angle(f),
# compression(B), spare(B), radial_len(H), az_spacing(B), radial_status(B),
# el_num(B), sector_cut_num(B), el_angle(f), radial_spot_blanking(B),
# az_index_mode(B), num_data_blks(H), vol_ptr(L), el_ptr(L), rad_ptr(L),
# moment_ptrs(9L) — 9 possible moment block pointers
MSG31_HDR_FMT = '>4sLHHfBBHBBBBfBBH'
MSG31_HDR_FIELDS = (
    'radar_id', 'ms_since_midnight', 'julian_date', 'az_num', 'az_angle',
    'compression', 'spare', 'radial_len', 'az_spacing', 'radial_status',
    'el_num', 'sector_cut_num', 'el_angle', 'radial_spot_blanking',
    'az_index_mode', 'num_data_blks',
)
MSG31_HDR_LEN = struct.calcsize(MSG31_HDR_FMT)

# Each data block starts with a preamble before the data fields.
# There are two preamble sizes in Message 31:
#   Constant blocks (RVOL, RELV, RRAD): block_id(4) + block_size(2) = 6 bytes
#   Moment data blocks (DREF, DVEL, etc): block_id(4) + block_size(2) + version(2) = 8 bytes
BLOCK_PREAMBLE_SIZE    = 6   # for RVOL, RELV, RRAD
MOMENT_PREAMBLE_SIZE   = 8   # for DREF, DVEL, DSW, DZDR, DPHI, DRHO, DCFP

# Following the fixed header fields: vol_ptr, el_ptr, rad_ptr, then up to 9 moment ptrs
# Each pointer is a 4-byte unsigned int (offset from start of message 31 body)
PTR_FMT = '>L'
PTR_SIZE = struct.calcsize(PTR_FMT)  # 4

# Radial status codes
RADIAL_STATUS = {
    0: 'Start of Elevation',
    1: 'Intermediate',
    2: 'End of Elevation',
    3: 'Start of Volume',
    4: 'End of Volume',
    5: 'Start of Elevation (cut)',
}

# Waveform type codes
WAVEFORM_TYPES = {
    1: 'Contiguous Surveillance',
    2: 'Contiguous Doppler (no ambiguity)',
    3: 'Contiguous Doppler (with ambiguity)',
    4: 'Batch',
    5: 'Staggered Pulse Pair',
}

# Data block type identifiers (4-char ASCII)
BLOCK_TYPES = {
    b'RVOL': 'Volume Constants',
    b'RELV': 'Elevation Constants',
    b'RRAD': 'Radial Constants',
    b'DREF': 'Reflectivity (REF)',
    b'DVEL': 'Velocity (VEL)',
    b'DSW ': 'Spectrum Width (SW)',
    b'DZDR': 'Differential Reflectivity (ZDR)',
    b'DPHI': 'Differential Phase (PHI)',
    b'DRHO': 'Correlation Coefficient (RHO)',
    b'DCFP': 'Clutter Filter Power (CFP)',
}

MOMENT_BLOCKS = {b'DREF', b'DVEL', b'DSW ', b'DZDR', b'DPHI', b'DRHO', b'DCFP'}


# ---------------------------------------------------------------------------
# Volume Constants block (RVOL)
# Preamble: block_id(4) + block_size(2) = 6 bytes (skipped before parsing)
# ---------------------------------------------------------------------------

# lat(f), lon(f), site_amsl(h), feedhorn_agl(H), calib_dbz(f),
# txpower_h(f), txpower_v(f), sys_zdr(f), phidp0(f), vcp(H), processing_status(H)
# Note: preceded by major(B) + minor(B) version fields after the 6-byte preamble
RVOL_FMT = '>BBffhHfffffHH'
RVOL_FIELDS = (
    'major', 'minor',
    'lat', 'lon', 'site_amsl', 'feedhorn_agl', 'calib_dbz',
    'txpower_h', 'txpower_v', 'sys_zdr', 'phidp0', 'vcp', 'processing_status',
)


# ---------------------------------------------------------------------------
# Elevation Constants block (RELV)
# Preamble: block_id(4) + block_size(2) = 6 bytes (skipped before parsing)
# ---------------------------------------------------------------------------

RELV_FMT = '>fH'
RELV_FIELDS = ('atmos_atten', 'calib_const')


# ---------------------------------------------------------------------------
# Radial Constants block (RRAD)
# Preamble: block_id(4) + block_size(2) = 6 bytes (skipped before parsing)
#
# Two versions exist — distinguished by block_size in the preamble:
#   v1 (size=20): unamb_range, horiz_noise, vert_noise, nyquist_vel, spare,
#                 calib_const_h, calib_const_v
#   v2 (size=28): adds radial_flags(H) after spare, and two more calib fields
#
# Unit notes (per ICD):
#   unamb_range: stored in 1/8 km units → divide by 8 to get km
#   nyquist_vel: stored in 0.01 m/s units → divide by 100 to get m/s
# ---------------------------------------------------------------------------

RRAD_V1_FMT = '>HffHHff'
RRAD_V1_FIELDS = (
    'unamb_range', 'horiz_noise', 'vert_noise',
    'nyquist_vel', 'spare', 'calib_const_h', 'calib_const_v',
)
RRAD_V1_SIZE = struct.calcsize(RRAD_V1_FMT)  # 20 bytes

RRAD_V2_FMT = '>HffHHHHff'
RRAD_V2_FIELDS = (
    'unamb_range', 'horiz_noise', 'vert_noise',
    'nyquist_vel', 'spare', 'radial_flags', 'spare2',
    'calib_const_h', 'calib_const_v',
)
RRAD_V2_SIZE = struct.calcsize(RRAD_V2_FMT)  # 28 bytes


# ---------------------------------------------------------------------------
# Moment data block header
# Preamble: block_id(4) + block_size(2) = 6 bytes (skipped before parsing)
# ---------------------------------------------------------------------------

# gate_count(H), first_gate(H), gate_width(H), tover(H), snr_threshold(H),
# spare(B), word_size(B), scale(f), offset(f)
MOMENT_HDR_FMT = '>HHHHHBBff'
MOMENT_HDR_FIELDS = (
    'gate_count', 'first_gate', 'gate_width', 'tover', 'snr_threshold',
    'spare', 'word_size', 'scale', 'offset',
)
MOMENT_HDR_LEN = struct.calcsize(MOMENT_HDR_FMT)


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------

def decode_msg31(raw_bytes: bytes) -> dict | None:
    """
    Decode a Message 31 radial from its raw bytes (including CTM + msg headers).
    Returns a dict with parsed fields, or None if parsing fails.
    """
    try:
        body = raw_bytes[MSG31_HDR_OFFSET:]

        # Parse fixed data header fields
        hdr_values = struct.unpack_from(MSG31_HDR_FMT, body, 0)
        hdr = dict(zip(MSG31_HDR_FIELDS, hdr_values))

        # radar_id is bytes — decode it
        hdr['radar_id'] = hdr['radar_id'].decode('ascii', errors='replace').strip()

        # Read the block pointers that follow the fixed header
        ptr_offset = MSG31_HDR_LEN
        vol_ptr  = struct.unpack_from(PTR_FMT, body, ptr_offset)[0];     ptr_offset += PTR_SIZE
        el_ptr   = struct.unpack_from(PTR_FMT, body, ptr_offset)[0];     ptr_offset += PTR_SIZE
        rad_ptr  = struct.unpack_from(PTR_FMT, body, ptr_offset)[0];     ptr_offset += PTR_SIZE

        num_moment_ptrs = hdr['num_data_blks'] - 3  # subtract vol, el, rad blocks
        moment_ptrs = []
        for _ in range(max(0, num_moment_ptrs)):
            moment_ptrs.append(struct.unpack_from(PTR_FMT, body, ptr_offset)[0])
            ptr_offset += PTR_SIZE

        # Decode Volume Constants block
        # Preamble: block_id(4) + block_size(2) = 6 bytes
        vol_consts = None
        if vol_ptr > 0 and vol_ptr + BLOCK_PREAMBLE_SIZE < len(body):
            vc_offset = vol_ptr + BLOCK_PREAMBLE_SIZE
            if vc_offset + struct.calcsize(RVOL_FMT) <= len(body):
                values = struct.unpack_from(RVOL_FMT, body, vc_offset)
                vol_consts = dict(zip(RVOL_FIELDS, values))

        # Decode Elevation Constants block
        el_consts = None
        if el_ptr > 0 and el_ptr + BLOCK_PREAMBLE_SIZE < len(body):
            ec_offset = el_ptr + BLOCK_PREAMBLE_SIZE
            if ec_offset + struct.calcsize(RELV_FMT) <= len(body):
                values = struct.unpack_from(RELV_FMT, body, ec_offset)
                el_consts = dict(zip(RELV_FIELDS, values))

        # Decode Radial Constants block
        # Detect v1 vs v2 from the block_size field in the preamble (bytes 4-5)
        rad_consts = None
        if rad_ptr > 0 and rad_ptr + BLOCK_PREAMBLE_SIZE < len(body):
            block_size = struct.unpack_from('>H', body, rad_ptr + 4)[0]
            rc_offset = rad_ptr + BLOCK_PREAMBLE_SIZE
            if block_size >= RRAD_V2_SIZE and rc_offset + RRAD_V2_SIZE <= len(body):
                values = struct.unpack_from(RRAD_V2_FMT, body, rc_offset)
                rad_consts = dict(zip(RRAD_V2_FIELDS, values))
            elif rc_offset + RRAD_V1_SIZE <= len(body):
                values = struct.unpack_from(RRAD_V1_FMT, body, rc_offset)
                rad_consts = dict(zip(RRAD_V1_FIELDS, values))

            # Apply ICD unit scaling
            if rad_consts:
                rad_consts['unamb_range_km'] = rad_consts['unamb_range'] / 8.0
                rad_consts['nyquist_vel_ms'] = rad_consts['nyquist_vel'] / 100.0

        # Decode moment block headers
        moments = {}
        for ptr in moment_ptrs:
            if ptr == 0 or ptr + MOMENT_PREAMBLE_SIZE >= len(body):
                continue
            block_id = body[ptr:ptr + 4]
            if block_id not in MOMENT_BLOCKS:
                continue
            name = block_id.decode('ascii', errors='replace').strip()
            mh_offset = ptr + MOMENT_PREAMBLE_SIZE
            if mh_offset + MOMENT_HDR_LEN <= len(body):
                values = struct.unpack_from(MOMENT_HDR_FMT, body, mh_offset)
                moments[name] = dict(zip(MOMENT_HDR_FIELDS, values))

        return {
            'hdr':        hdr,
            'vol_consts': vol_consts,
            'el_consts':  el_consts,
            'rad_consts': rad_consts,
            'moments':    moments,
        }

    except Exception:
        return None
