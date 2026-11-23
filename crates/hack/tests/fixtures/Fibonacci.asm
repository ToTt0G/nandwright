// Writes the first RAM[0] Fibonacci numbers to RAM[100], RAM[101], ...
// (0, 1, 1, 2, 3, 5, ...; values wrap around at 16 bits).
    @cur
    M=0
    @next
    M=1
    @100
    D=A
    @ptr
    M=D
    @R0
    D=M
    @left
    M=D
(LOOP)
    @left
    D=M
    @END
    D;JLE
    @cur
    D=M
    @ptr
    A=M
    M=D           // *ptr = cur
    @next
    D=M
    @cur
    D=D+M         // D = cur + next
    @sum
    M=D
    @next
    D=M
    @cur
    M=D           // cur = next
    @sum
    D=M
    @next
    M=D           // next = sum
    @ptr
    M=M+1
    @left
    M=M-1
    @LOOP
    0;JMP
(END)
    @END
    0;JMP
