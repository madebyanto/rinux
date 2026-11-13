.section .text
.code64

.global context_switch
context_switch:
    mov [rdi + 0], rsp
    pushfq
    pop rax
    mov [rdi + 8], rax
    mov [rdi + 16], rbx
    mov [rdi + 24], rbp
    mov [rdi + 32], r12
    mov [rdi + 40], r13
    mov [rdi + 48], r14
    mov [rdi + 56], r15

    mov rsp, [rsi + 0]
    mov rbx, [rsi + 16]
    mov rbp, [rsi + 24]
    mov r12, [rsi + 32]
    mov r13, [rsi + 40]
    mov r14, [rsi + 48]
    mov r15, [rsi + 56]
    mov rax, [rsi + 8]
    push rax
    mov eax, 0
    popfq
    ret

.global task_start
task_start:
    sti
    call rbx
    cli
    call task_finished
    hlt

.global user_task_start
user_task_start:
    push r15
    push r13
    push rbp
    push r14
    push r12
    xor ebx, ebx
    xor ebp, ebp
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    iretq
