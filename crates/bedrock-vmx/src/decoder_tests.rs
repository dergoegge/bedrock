// SPDX-License-Identifier: GPL-2.0

use super::*;

#[test]
fn test_mov_load_simple() {
    // mov eax, [rbx] = 8B 03
    let instr = decode_instruction(&[0x8B, 0x03]).unwrap();
    assert_eq!(instr.length, 2);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0); // EAX
    assert_eq!(instr.operand_size, 4);
}

#[test]
fn test_mov_store_simple() {
    // mov [rbx], eax = 89 03
    let instr = decode_instruction(&[0x89, 0x03]).unwrap();
    assert_eq!(instr.length, 2);
    assert_eq!(instr.operation, MemoryOperation::Store);
    assert_eq!(instr.register, 0); // EAX
    assert_eq!(instr.operand_size, 4);
}

#[test]
fn test_mov_with_rex_w() {
    // mov rax, [rbx] = 48 8B 03 (REX.W prefix)
    let instr = decode_instruction(&[0x48, 0x8B, 0x03]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0); // RAX
    assert_eq!(instr.operand_size, 8);
}

#[test]
fn test_mov_with_rex_r() {
    // mov r8d, [rbx] = 44 8B 03 (REX.R prefix)
    let instr = decode_instruction(&[0x44, 0x8B, 0x03]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 8); // R8D
    assert_eq!(instr.operand_size, 4);
}

#[test]
fn test_mov_with_displacement() {
    // mov eax, [rbx+0x10] = 8B 43 10 (disp8)
    let instr = decode_instruction(&[0x8B, 0x43, 0x10]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0);
    assert_eq!(instr.operand_size, 4);
}

#[test]
fn test_mov_with_disp32() {
    // mov eax, [rbx+0x12345678] = 8B 83 78 56 34 12 (disp32)
    let instr = decode_instruction(&[0x8B, 0x83, 0x78, 0x56, 0x34, 0x12]).unwrap();
    assert_eq!(instr.length, 6);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0);
}

#[test]
fn test_mov_with_sib() {
    // mov eax, [rsp] = 8B 04 24 (SIB byte needed for RSP-based addressing)
    let instr = decode_instruction(&[0x8B, 0x04, 0x24]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0);
}

#[test]
fn test_mov_rip_relative() {
    // mov eax, [rip+0x12345678] = 8B 05 78 56 34 12
    let instr = decode_instruction(&[0x8B, 0x05, 0x78, 0x56, 0x34, 0x12]).unwrap();
    assert_eq!(instr.length, 6);
    assert_eq!(instr.operation, MemoryOperation::Load);
}

#[test]
fn test_mov_byte() {
    // mov al, [rbx] = 8A 03
    let instr = decode_instruction(&[0x8A, 0x03]).unwrap();
    assert_eq!(instr.length, 2);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.operand_size, 1);
}

#[test]
fn test_movzx_byte() {
    // movzx eax, byte ptr [rbx] = 0F B6 03
    let instr = decode_instruction(&[0x0F, 0xB6, 0x03]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.register, 0);
    assert_eq!(instr.operand_size, 1); // Source is byte
}

#[test]
fn test_movzx_word() {
    // movzx eax, word ptr [rbx] = 0F B7 03
    let instr = decode_instruction(&[0x0F, 0xB7, 0x03]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.operand_size, 2); // Source is word
}

#[test]
fn test_operand_size_prefix() {
    // mov ax, [rbx] = 66 8B 03 (16-bit operand)
    let instr = decode_instruction(&[0x66, 0x8B, 0x03]).unwrap();
    assert_eq!(instr.length, 3);
    assert_eq!(instr.operation, MemoryOperation::Load);
    assert_eq!(instr.operand_size, 2);
}

#[test]
fn test_unsupported_opcode() {
    // NOP = 90
    let result = decode_instruction(&[0x90]);
    assert!(matches!(result, Err(DecodeError::UnsupportedOpcode)));
}

#[test]
fn test_register_to_register() {
    // mov eax, ebx = 8B C3 (mod=11, reg-to-reg, not memory)
    let result = decode_instruction(&[0x8B, 0xC3]);
    assert!(matches!(result, Err(DecodeError::UnsupportedAddressing)));
}

#[test]
fn test_buffer_too_short() {
    let result = decode_instruction(&[0x8B]);
    assert!(matches!(result, Err(DecodeError::BufferTooShort)));
}
