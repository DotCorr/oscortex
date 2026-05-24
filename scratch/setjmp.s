.intel_syntax noprefix
.global _setjmp
.global longjmp
.text

_setjmp:
    mov [rdi + 0],  rbx
    lea rax, [rsp + 8]
    mov [rdi + 8],  rax
    mov [rdi + 16], rbp
    mov [rdi + 24], r12
    mov [rdi + 32], r13
    mov [rdi + 40], r14
    mov [rdi + 48], r15
    mov rax, [rsp]
    mov [rdi + 56], rax
    xor rax, rax
    ret

longjmp:
    mov rbx, [rdi + 0]
    mov rsp, [rdi + 8]
    mov rbp, [rdi + 16]
    mov r12, [rdi + 24]
    mov r13, [rdi + 32]
    mov r14, [rdi + 40]
    mov r15, [rdi + 48]
    mov rdx, [rdi + 56]
    mov rax, rsi
    test rax, rax
    jnz .Lnon_zero
    mov rax, 1
.Lnon_zero:
    jmp rdx
