// Benchmark: an endless loop that adds RAM[0..1023] into `sum` over and over,
// mixing memory reads, writes, arithmetic and jumps.
(LOOP)
    @i
    D=M
    @1023
    D=D&A
    @i
    M=D+1
    A=D
    D=M
    @sum
    M=D+M
    @LOOP
    0;JMP
