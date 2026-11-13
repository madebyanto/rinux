.intel_syntax noprefix

.section .text
.global _start
_start:
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end - message
    syscall

    sub rsp, 16

read_loop:
    mov eax, 0
    xor edi, edi
    mov rsi, rsp
    mov edx, 1
    syscall
    cmp rax, 1
    jne read_loop

    movzx eax, byte ptr [rsp]
    cmp al, 0x1B
    je exit
    cmp al, 0x21
    je fault

    mov byte ptr [rsp], al
    mov byte ptr [rsp + 1], 0x0A
    mov eax, 1
    mov edi, 1
    mov rsi, rsp
    mov edx, 2
    syscall
    jmp read_loop

exit:
    mov eax, 60
    xor edi, edi
    syscall

fault:
    mov qword ptr [0], 0
    jmp fault

.section .rodata
message:
    .ascii "hello from ring 3!\n"
message_end:
