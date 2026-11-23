// Counts RAM[0] down to zero, writing each value to RAM[100], RAM[101], ...
// RAM[1] receives the number of values written.
    @100
    D=A
    @ptr
    M=D
    @R1
    M=0
(LOOP)
    @R0
    D=M
    @DONE
    D;JLE         // stop at zero (or if RAM[0] started negative)
    @ptr
    A=M
    M=D           // *ptr = n
    @ptr
    M=M+1
    @R1
    M=M+1
    @R0
    M=M-1
    @LOOP
    0;JMP
(DONE)
    @DONE
    0;JMP
