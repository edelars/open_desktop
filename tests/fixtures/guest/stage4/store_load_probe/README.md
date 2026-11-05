# Stage4StoreLoadProbeV1

The builder assembles this fixture using the fixed stage4 page-table declarations
and this profile's guest instruction/data section. The source entry executes
`MOV [RBX],RCX; MOV RAX,[RBX]; OUT DX,AL; HLT`. Raw data at GPA `0x9000`
contains the initial qword; GPA `0x9008` contains the store-value qword. The
loader uses the latter only to initialize RCX in the documented handoff. It
does not predict or insert UART output.

The guest must retire all four instructions for a successful run. The bounded
image remains `0x9000` bytes at GPA `0x1000`, with the S4-32a Long64/CPL0 page
tables, code, data, stack and UART layout.
