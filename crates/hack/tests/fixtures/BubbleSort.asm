// Sorts RAM[100] .. RAM[100 + RAM[0] - 1] in ascending order with a bubble
// sort. Elements are compared by subtraction, so they should differ by less
// than 32768.
(PASS)
    @swapped
    M=0
    @100
    D=A
    @p
    M=D           // p = first element
    @R0
    D=M
    @100
    D=D+A
    @end
    M=D           // end = one past the last element
(INNER)
    @p
    D=M+1
    @end
    D=D-M
    @CHECK
    D;JGE         // p + 1 >= end: pass finished
    @p
    A=M
    D=M
    A=A+1
    D=D-M         // D = a[p] - a[p+1]
    @NOSWAP
    D;JLE
    @p
    A=M
    D=M
    @tmp
    M=D           // tmp = a[p]
    @p
    A=M+1
    D=M
    A=A-1
    M=D           // a[p] = a[p+1]
    @tmp
    D=M
    @p
    A=M+1
    M=D           // a[p+1] = tmp
    @swapped
    M=1
(NOSWAP)
    @p
    M=M+1
    @INNER
    0;JMP
(CHECK)
    @swapped
    D=M
    @PASS
    D;JNE
(END)
    @END
    0;JMP
