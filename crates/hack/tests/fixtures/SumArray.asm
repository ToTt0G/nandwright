// RAM[1] = RAM[100] + RAM[101] + ... + RAM[100 + RAM[0] - 1]
    @R1
    M=0
    @i
    M=0
(LOOP)
    @i
    D=M
    @R0
    D=D-M
    @END
    D;JGE         // i >= n: done
    @i
    D=M
    @100
    A=D+A
    D=M           // D = RAM[100 + i]
    @R1
    M=D+M
    @i
    M=M+1
    @LOOP
    0;JMP
(END)
    @END
    0;JMP
