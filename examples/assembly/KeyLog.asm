// Records key presses. Each time a key goes down its code is appended to
// RAM[200], RAM[201], ...; after RAM[0] keys the program halts.
// RAM[1] counts the keys recorded so far.
    @200
    D=A
    @ptr
    M=D
    @R1
    M=0
(WAITDOWN)
    @R1
    D=M
    @R0
    D=D-M
    @HALT
    D;JGE
    @KBD
    D=M
    @WAITDOWN
    D;JEQ         // no key yet
    @ptr
    A=M
    M=D           // record it
    @ptr
    M=M+1
    @R1
    M=M+1
(WAITUP)
    @KBD
    D=M
    @WAITUP
    D;JNE         // wait for the key to be released
    @WAITDOWN
    0;JMP
(HALT)
    @HALT
    0;JMP
