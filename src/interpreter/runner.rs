// TODO: WebAssembly code must be validated before it can be instantiated and executed.
// WebAssembly is designed to allow decoding and validation to be performed in a single linear pass through a WebAssembly module,
// and to enable many parts of decoding and validation to be performed concurrently.
// => Interpreter is written in assumption that code has been validated
// (important https://github.com/sunfishcode/wasm-reference-manual/blob/master/WebAssembly.md#code-section)

// Externals:
// to call function: list of imported functions + list of functions
// to access globals: list of imported globals + list of globals
// to access linear memory: list of imported regions + list of regions

use std::sync::Weak;
use std::mem;
use std::ops;
use std::u32;
use std::collections::VecDeque;
use super::super::elements::{Module, Opcode, BlockType, FunctionType};
use interpreter::Error;
use interpreter::module::{ModuleInstance, ItemIndex};
use interpreter::value::{RuntimeValue, TryInto, WrapInto, TryTruncateInto, ExtendInto, TransmuteInto,
	ArithmeticOps, Integer, Float};

const DEFAULT_MEMORY_INDEX: u32 = 0;

pub struct Interpreter;

/// Function execution context.
struct FunctionContext<'a> {
	/// Module instance.
	module: &'a mut ModuleInstance,
	/// Values stack.
	value_stack: &'a mut VecDeque<RuntimeValue>,
	/// Blocks frames stack.
	frame_stack: &'a mut VecDeque<BlockFrame>,
	/// Local function variables.
	locals: Vec<RuntimeValue>,
	/// Current instruction position.
	position: usize,
}

#[derive(Debug, Clone)]
enum InstructionOutcome {
	/// Continue with current instruction.
	RunInstruction,
	/// Continue with next instruction.
	RunNextInstruction,
	/// Pop given number of stack frames.
	PopFrame(usize),
	/// Return from current function block.
	Return,
}

#[derive(Debug, Clone)]
struct BlockFrame {
	// A label for reference from branch instructions.
	position: usize,
	// A limit integer value, which is an index into the value stack indicating where to reset it to on a branch to that label.
	value_limit: usize,
	// A signature, which is a block signature type indicating the number and types of result values of the region.
	signature: BlockType,
}

impl Interpreter {
	pub fn run_function(function: &FunctionType, body: &[Opcode], args: &[RuntimeValue]) -> Result<Option<RuntimeValue>, Error> {
		// prepare execution context
		let mut module = ModuleInstance::new(Weak::default(), Module::default()).unwrap();
		let mut value_stack = VecDeque::new();
		let mut frame_stack = VecDeque::new();
		let mut context = FunctionContext::new(&mut module, &mut value_stack, &mut frame_stack, function, body, args)?;

		let block_type = match function.return_type() {
			Some(value_type) => BlockType::Value(value_type),
			None => BlockType::NoResult,
		};
		Interpreter::execute_block(&mut context, block_type.clone(), body)?;
		match block_type {
			BlockType::Value(_) => Ok(Some(context.pop_value()?)),
			BlockType::NoResult => Ok(None),
		}
	}

	fn run_instruction(context: &mut FunctionContext, opcode: &Opcode) -> Result<InstructionOutcome, Error> {
		match opcode {
			&Opcode::Unreachable => Interpreter::run_unreachable(context),
			&Opcode::Nop => Interpreter::run_nop(context),
			&Opcode::Block(block_type, ref ops) => Interpreter::run_block(context, block_type, ops.elements()),
			&Opcode::Loop(block_type, ref ops) => Interpreter::run_loop(context, block_type, ops.elements()),
			&Opcode::If(block_type, ref ops) => Interpreter::run_if(context, block_type, ops.elements()),
			&Opcode::Else => Interpreter::run_else(context),
			&Opcode::End => Interpreter::run_end(context),
			&Opcode::Br(idx) => Interpreter::run_br(context, idx),
			&Opcode::BrIf(idx) => Interpreter::run_br_if(context, idx),
			&Opcode::BrTable(ref table, default) => Interpreter::run_br_table(context, table, default),
			&Opcode::Return => Interpreter::run_return(context),

			&Opcode::Call(index) => Interpreter::run_call(context, index),
			&Opcode::CallIndirect(index, reserved) => Interpreter::run_call_indirect(context, index),

			&Opcode::Drop => Interpreter::run_drop(context),
			&Opcode::Select => Interpreter::run_select(context),

			&Opcode::GetLocal(index) => Interpreter::run_get_local(context, index),
			&Opcode::SetLocal(index) => Interpreter::run_set_local(context, index),
			&Opcode::TeeLocal(index) => Interpreter::run_tee_local(context, index),
			&Opcode::GetGlobal(index) => Interpreter::run_get_global(context, index),
			&Opcode::SetGlobal(index) => Interpreter::run_set_global(context, index),

			&Opcode::I32Load(offset, align) => Interpreter::run_load::<i32>(context, offset, align),
			&Opcode::I64Load(offset, align) => Interpreter::run_load::<i64>(context, offset, align),
			&Opcode::F32Load(offset, align) => Interpreter::run_load::<f32>(context, offset, align),
			&Opcode::F64Load(offset, align) => Interpreter::run_load::<f64>(context, offset, align),
			&Opcode::I32Load8S(offset, align) => Interpreter::run_load_extend::<i8, i32>(context, offset, align),
			&Opcode::I32Load8U(offset, align) => Interpreter::run_load_extend::<u8, i32>(context, offset, align),
			&Opcode::I32Load16S(offset, align) => Interpreter::run_load_extend::<i16, i32>(context, offset, align),
			&Opcode::I32Load16U(offset, align) => Interpreter::run_load_extend::<u16, i32>(context, offset, align),
			&Opcode::I64Load8S(offset, align) => Interpreter::run_load_extend::<i8, i64>(context, offset, align),
			&Opcode::I64Load8U(offset, align) => Interpreter::run_load_extend::<u8, i64>(context, offset, align),
			&Opcode::I64Load16S(offset, align) => Interpreter::run_load_extend::<i16, i64>(context, offset, align),
			&Opcode::I64Load16U(offset, align) => Interpreter::run_load_extend::<u16, i64>(context, offset, align),
			&Opcode::I64Load32S(offset, align) => Interpreter::run_load_extend::<i32, i64>(context, offset, align),
			&Opcode::I64Load32U(offset, align) => Interpreter::run_load_extend::<u32, i64>(context, offset, align),

			&Opcode::I32Store(offset, align) => Interpreter::run_store::<i32>(context, offset, align),
			&Opcode::I64Store(offset, align) => Interpreter::run_store::<i64>(context, offset, align),
			&Opcode::F32Store(offset, align) => Interpreter::run_store::<f32>(context, offset, align),
			&Opcode::F64Store(offset, align) => Interpreter::run_store::<f64>(context, offset, align),
			&Opcode::I32Store8(offset, align) => Interpreter::run_store_wrap::<i32, i8>(context, offset, align),
			&Opcode::I32Store16(offset, align) => Interpreter::run_store_wrap::<i32, i16>(context, offset, align),
			&Opcode::I64Store8(offset, align) => Interpreter::run_store_wrap::<i64, i8>(context, offset, align),
			&Opcode::I64Store16(offset, align) => Interpreter::run_store_wrap::<i64, i16>(context, offset, align),
			&Opcode::I64Store32(offset, align) => Interpreter::run_store_wrap::<i64, i32>(context, offset, align),

			&Opcode::CurrentMemory(_) => Interpreter::run_current_memory(context),
			&Opcode::GrowMemory(_) => Interpreter::run_grow_memory(context),

			&Opcode::I32Const(val) => Interpreter::run_const(context, val.into()),
			&Opcode::I64Const(val) => Interpreter::run_const(context, val.into()),
			&Opcode::F32Const(val) => Interpreter::run_const(context, RuntimeValue::decode_f32(val)),
			&Opcode::F64Const(val) => Interpreter::run_const(context, RuntimeValue::decode_f64(val)),

			&Opcode::I32Eqz => Interpreter::run_eqz::<i32>(context),
			&Opcode::I32Eq => Interpreter::run_eq::<i32>(context),
			&Opcode::I32Ne => Interpreter::run_ne::<i32>(context),
			&Opcode::I32LtS => Interpreter::run_lt::<i32>(context),
			&Opcode::I32LtU => Interpreter::run_lt::<u32>(context),
			&Opcode::I32GtS => Interpreter::run_gt::<i32>(context),
			&Opcode::I32GtU => Interpreter::run_gt::<u32>(context),
			&Opcode::I32LeS => Interpreter::run_lte::<i32>(context),
			&Opcode::I32LeU => Interpreter::run_lte::<u32>(context),
			&Opcode::I32GeS => Interpreter::run_gte::<i32>(context),
			&Opcode::I32GeU => Interpreter::run_gte::<u32>(context),

			&Opcode::I64Eqz => Interpreter::run_eqz::<i64>(context),
			&Opcode::I64Eq => Interpreter::run_eq::<i64>(context),
			&Opcode::I64Ne => Interpreter::run_ne::<i64>(context),
			&Opcode::I64LtS => Interpreter::run_lt::<i64>(context),
			&Opcode::I64LtU => Interpreter::run_lt::<u64>(context),
			&Opcode::I64GtS => Interpreter::run_gt::<i64>(context),
			&Opcode::I64GtU => Interpreter::run_gt::<u64>(context),
			&Opcode::I64LeS => Interpreter::run_lte::<i64>(context),
			&Opcode::I64LeU => Interpreter::run_lte::<u64>(context),
			&Opcode::I64GeS => Interpreter::run_gte::<i64>(context),
			&Opcode::I64GeU => Interpreter::run_gte::<u64>(context),

			&Opcode::F32Eq => Interpreter::run_eq::<f32>(context),
			&Opcode::F32Ne => Interpreter::run_ne::<f32>(context),
			&Opcode::F32Lt => Interpreter::run_lt::<f32>(context),
			&Opcode::F32Gt => Interpreter::run_gt::<f32>(context),
			&Opcode::F32Le => Interpreter::run_lte::<f32>(context),
			&Opcode::F32Ge => Interpreter::run_gte::<f32>(context),

			&Opcode::F64Eq => Interpreter::run_eq::<f64>(context),
			&Opcode::F64Ne => Interpreter::run_ne::<f64>(context),
			&Opcode::F64Lt => Interpreter::run_lt::<f64>(context),
			&Opcode::F64Gt => Interpreter::run_gt::<f64>(context),
			&Opcode::F64Le => Interpreter::run_lte::<f64>(context),
			&Opcode::F64Ge => Interpreter::run_gte::<f64>(context),

			&Opcode::I32Clz => Interpreter::run_clz::<i32>(context),
			&Opcode::I32Ctz => Interpreter::run_ctz::<i32>(context),
			&Opcode::I32Popcnt => Interpreter::run_popcnt::<i32>(context),
			&Opcode::I32Add => Interpreter::run_add::<i32>(context),
			&Opcode::I32Sub => Interpreter::run_sub::<i32>(context),
			&Opcode::I32Mul => Interpreter::run_mul::<i32>(context),
			&Opcode::I32DivS => Interpreter::run_div::<i32, i32>(context),
			&Opcode::I32DivU => Interpreter::run_div::<i32, u32>(context),
			&Opcode::I32RemS => Interpreter::run_rem::<i32, i32>(context),
			&Opcode::I32RemU => Interpreter::run_rem::<i32, u32>(context),
			&Opcode::I32And => Interpreter::run_and::<i32>(context),
			&Opcode::I32Or => Interpreter::run_or::<i32>(context),
			&Opcode::I32Xor => Interpreter::run_xor::<i32>(context),
			&Opcode::I32Shl => Interpreter::run_shl::<i32>(context),
			&Opcode::I32ShrS => Interpreter::run_shr::<i32, i32>(context),
			&Opcode::I32ShrU => Interpreter::run_shr::<i32, u32>(context),
			&Opcode::I32Rotl => Interpreter::run_rotl::<i32>(context),
			&Opcode::I32Rotr => Interpreter::run_rotr::<i32>(context),

			&Opcode::I64Clz => Interpreter::run_clz::<i64>(context),
			&Opcode::I64Ctz => Interpreter::run_ctz::<i64>(context),
			&Opcode::I64Popcnt => Interpreter::run_popcnt::<i64>(context),
			&Opcode::I64Add => Interpreter::run_add::<i64>(context),
			&Opcode::I64Sub => Interpreter::run_sub::<i64>(context),
			&Opcode::I64Mul => Interpreter::run_mul::<i64>(context),
			&Opcode::I64DivS => Interpreter::run_div::<i64, i64>(context),
			&Opcode::I64DivU => Interpreter::run_div::<i64, u64>(context),
			&Opcode::I64RemS => Interpreter::run_rem::<i64, i64>(context),
			&Opcode::I64RemU => Interpreter::run_rem::<i64, u64>(context),
			&Opcode::I64And => Interpreter::run_and::<i64>(context),
			&Opcode::I64Or => Interpreter::run_or::<i64>(context),
			&Opcode::I64Xor => Interpreter::run_xor::<i64>(context),
			&Opcode::I64Shl => Interpreter::run_shl::<i64>(context),
			&Opcode::I64ShrS => Interpreter::run_shr::<i64, i64>(context),
			&Opcode::I64ShrU => Interpreter::run_shr::<i64, u64>(context),
			&Opcode::I64Rotl => Interpreter::run_rotl::<i64>(context),
			&Opcode::I64Rotr => Interpreter::run_rotr::<i64>(context),

			&Opcode::F32Abs => Interpreter::run_abs::<f32>(context),
			&Opcode::F32Neg => Interpreter::run_neg::<f32>(context),
			&Opcode::F32Ceil => Interpreter::run_ceil::<f32>(context),
			&Opcode::F32Floor => Interpreter::run_floor::<f32>(context),
			&Opcode::F32Trunc => Interpreter::run_trunc::<f32>(context),
			&Opcode::F32Nearest => Interpreter::run_nearest::<f32>(context),
			&Opcode::F32Sqrt => Interpreter::run_sqrt::<f32>(context),
			&Opcode::F32Add => Interpreter::run_add::<f32>(context),
			&Opcode::F32Sub => Interpreter::run_sub::<f32>(context),
			&Opcode::F32Mul => Interpreter::run_mul::<f32>(context),
			&Opcode::F32Div => Interpreter::run_div::<f32, f32>(context),
			&Opcode::F32Min => Interpreter::run_min::<f32>(context),
			&Opcode::F32Max => Interpreter::run_max::<f32>(context),
			&Opcode::F32Copysign => Interpreter::run_copysign::<f32>(context),

			&Opcode::F64Abs => Interpreter::run_abs::<f64>(context),
			&Opcode::F64Neg => Interpreter::run_neg::<f64>(context),
			&Opcode::F64Ceil => Interpreter::run_ceil::<f64>(context),
			&Opcode::F64Floor => Interpreter::run_floor::<f64>(context),
			&Opcode::F64Trunc => Interpreter::run_trunc::<f64>(context),
			&Opcode::F64Nearest => Interpreter::run_nearest::<f64>(context),
			&Opcode::F64Sqrt => Interpreter::run_sqrt::<f64>(context),
			&Opcode::F64Add => Interpreter::run_add::<f64>(context),
			&Opcode::F64Sub => Interpreter::run_sub::<f64>(context),
			&Opcode::F64Mul => Interpreter::run_mul::<f64>(context),
			&Opcode::F64Div => Interpreter::run_div::<f64, f64>(context),
			&Opcode::F64Min => Interpreter::run_min::<f64>(context),
			&Opcode::F64Max => Interpreter::run_max::<f64>(context),
			&Opcode::F64Copysign => Interpreter::run_copysign::<f64>(context),

			&Opcode::I32WarpI64 => Interpreter::run_wrap::<i64, i32>(context),
			&Opcode::I32TruncSF32 => Interpreter::run_trunc_to_int::<f32, i32, i32>(context),
			&Opcode::I32TruncUF32 => Interpreter::run_trunc_to_int::<f32, u32, i32>(context),
			&Opcode::I32TruncSF64 => Interpreter::run_trunc_to_int::<f64, i32, i32>(context),
			&Opcode::I32TruncUF64 => Interpreter::run_trunc_to_int::<f64, u32, i32>(context),
			&Opcode::I64ExtendSI32 => Interpreter::run_extend::<i32, i64, i64>(context),
			&Opcode::I64ExtendUI32 => Interpreter::run_extend::<u32, u64, i64>(context),
			&Opcode::I64TruncSF32 => Interpreter::run_trunc_to_int::<f32, i64, i64>(context),
			&Opcode::I64TruncUF32 => Interpreter::run_trunc_to_int::<f32, u64, i64>(context),
			&Opcode::I64TruncSF64 => Interpreter::run_trunc_to_int::<f64, i64, i64>(context),
			&Opcode::I64TruncUF64 => Interpreter::run_trunc_to_int::<f64, u64, i64>(context),
			&Opcode::F32ConvertSI32 => Interpreter::run_extend::<i32, f32, f32>(context),
			&Opcode::F32ConvertUI32 => Interpreter::run_extend::<u32, f32, f32>(context),
			&Opcode::F32ConvertSI64 => Interpreter::run_wrap::<i64, f32>(context),
			&Opcode::F32ConvertUI64 => Interpreter::run_wrap::<u64, f32>(context),
			&Opcode::F32DemoteF64 => Interpreter::run_wrap::<f64, f32>(context),
			&Opcode::F64ConvertSI32 => Interpreter::run_extend::<i32, f64, f64>(context),
			&Opcode::F64ConvertUI32 => Interpreter::run_extend::<u32, f64, f64>(context),
			&Opcode::F64ConvertSI64 => Interpreter::run_extend::<i64, f64, f64>(context),
			&Opcode::F64ConvertUI64 => Interpreter::run_extend::<u64, f64, f64>(context),
			&Opcode::F64PromoteF32 => Interpreter::run_extend::<f32, f64, f64>(context),

			&Opcode::I32ReinterpretF32 => Interpreter::run_reinterpret::<f32, i32>(context),
			&Opcode::I64ReinterpretF64 => Interpreter::run_reinterpret::<f64, i64>(context),
			&Opcode::F32ReinterpretI32 => Interpreter::run_reinterpret::<i32, f32>(context),
			&Opcode::F64ReinterpretI64 => Interpreter::run_reinterpret::<i64, f64>(context),
		}
	}

	fn run_unreachable(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		Err(Error::Trap)
	}

	fn run_nop(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		Ok(InstructionOutcome::RunNextInstruction)
	}

	fn run_block(context: &mut FunctionContext, block_type: BlockType, body: &[Opcode]) -> Result<InstructionOutcome, Error> {
		let frame_position = context.position + 1;
		context.push_frame(frame_position, block_type.clone())?;
		Interpreter::execute_block(context, block_type, body)
	}

	fn run_loop(context: &mut FunctionContext, block_type: BlockType, body: &[Opcode]) -> Result<InstructionOutcome, Error> {
		let frame_position = context.position;
		context.push_frame(frame_position, block_type.clone())?;
		Interpreter::execute_block(context, block_type, body)
	}

	fn run_if(context: &mut FunctionContext, block_type: BlockType, body: &[Opcode]) -> Result<InstructionOutcome, Error> {
		let body_len = body.len();
		let else_index = body.iter().position(|op| *op == Opcode::Else).unwrap_or(body_len - 1);
		let (begin_index, end_index) = if context.pop_value_as()? {
			(0, else_index + 1)
		} else {
			(else_index + 1, body_len)
		};

		if begin_index != end_index {
			let frame_position = context.position + 1;
			context.push_frame(frame_position, block_type.clone())?;
			Interpreter::execute_block(context, block_type, &body[begin_index..end_index])
		} else {
			Ok(InstructionOutcome::RunNextInstruction)
		}
	}

	fn run_else(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		Ok(InstructionOutcome::PopFrame(0))
	}

	fn run_end(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		Ok(InstructionOutcome::PopFrame(0))
	}

	fn run_br(context: &mut FunctionContext, label_idx: u32) -> Result<InstructionOutcome, Error> {
		Ok(InstructionOutcome::PopFrame(label_idx as usize))
	}

	fn run_br_if(context: &mut FunctionContext, label_idx: u32) -> Result<InstructionOutcome, Error> {
		if context.pop_value_as()? {
			Ok(InstructionOutcome::PopFrame(label_idx as usize))
		} else {
			Ok(InstructionOutcome::RunNextInstruction)
		}
	}

	fn run_br_table(context: &mut FunctionContext, table: &Vec<u32>, default: u32) -> Result<InstructionOutcome, Error> {
		let index: u32 = context.pop_value_as()?;
		Ok(InstructionOutcome::PopFrame(table.get(index as usize).cloned().unwrap_or(default) as usize))
	}

	fn run_return(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		Ok(InstructionOutcome::Return)
	}

	fn run_call(context: &mut FunctionContext, func_idx: u32) -> Result<InstructionOutcome, Error> {
		Err(Error::NotImplemented)
	}

	fn run_call_indirect(context: &mut FunctionContext, type_idx: u32) -> Result<InstructionOutcome, Error> {
		Err(Error::NotImplemented)
	}

	fn run_drop(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		context
			.pop_value()
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_select(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		context
			.pop_value_triple()
			.and_then(|(left, mid, right)|
				match (left, mid, right.try_into()) {
					(left, mid, Ok(condition)) => Ok((left, mid, condition)),
					_ => Err(Error::ValueStack("expected to get int value from stack".into()))
				}
			)
			.map(|(left, mid, condition)| if condition { left } else { mid })
			.map(|val| context.push_value(val))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_get_local(context: &mut FunctionContext, index: u32) -> Result<InstructionOutcome, Error> {
		context.get_local(index as usize)
			.map(|value| value.clone())
			.map(|value| context.push_value(value))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_set_local(context: &mut FunctionContext, index: u32) -> Result<InstructionOutcome, Error> {
		let arg = context.pop_value()?;
		context.set_local(index as usize, arg)
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_tee_local(context: &mut FunctionContext, index: u32) -> Result<InstructionOutcome, Error> {
		let arg = context.top_value()?.clone();
		context.set_local(index as usize, arg)
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_get_global(context: &mut FunctionContext, index: u32) -> Result<InstructionOutcome, Error> {
		context.module()
			.global(ItemIndex::IndexSpace(index))
			.and_then(|g| context.push_value(g.get()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_set_global(context: &mut FunctionContext, index: u32) -> Result<InstructionOutcome, Error> {
		context
			.pop_value()
			.and_then(|v| context.module().global(ItemIndex::IndexSpace(index)).and_then(|g| g.set(v)))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_load<T>(context: &mut FunctionContext, offset: u32, align: u32) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> {
		context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.and_then(|m| m.get(effective_address(offset, align)?, 4))
			.map(|b| from_little_endian_bytes::<T>(&b))
			.and_then(|n| context.push_value(n.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_load_extend<T, U>(context: &mut FunctionContext, offset: u32, align: u32) -> Result<InstructionOutcome, Error>
		where T: ExtendInto<U>, RuntimeValue: From<U> {
		let stack_value: U = context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.and_then(|m| m.get(effective_address(offset, align)?, mem::size_of::<T>()))
			.map(|b| from_little_endian_bytes::<T>(&b))
			.map(|v| v.extend_into())?;
		context
			.push_value(stack_value.into())
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_store<T>(context: &mut FunctionContext, offset: u32, align: u32) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error> {
		let stack_value = context
			.pop_value_as::<T>()
			.map(|n| to_little_endian_bytes::<T>(n))?;
		context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.and_then(|m| m.set(effective_address(offset, align)?, &stack_value))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_store_wrap<T, U>(context: &mut FunctionContext, offset: u32, align: u32) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: WrapInto<U> {
		let stack_value: T = context.pop_value().and_then(|v| v.try_into())?;
		let stack_value = to_little_endian_bytes::<U>(stack_value.wrap_into());
		context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.and_then(|m| m.set(effective_address(offset, align)?, &stack_value))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_current_memory(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.map(|m| m.size())
			.and_then(|s| context.push_value(RuntimeValue::I64(s as i64)))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_grow_memory(context: &mut FunctionContext) -> Result<InstructionOutcome, Error> {
		let pages: u32 = context.pop_value_as()?;
		context.module()
			.memory(ItemIndex::IndexSpace(DEFAULT_MEMORY_INDEX))
			.and_then(|m| m.grow(pages))
			.and_then(|m| context.push_value(RuntimeValue::I32(m as i32)))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_const(context: &mut FunctionContext, val: RuntimeValue) -> Result<InstructionOutcome, Error> {
		context
			.push_value(val)
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_eqz<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialEq<T> + Default {
		context
			.pop_value_as::<T>()
			.map(|v| RuntimeValue::I32(if v == Default::default() { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_eq<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialEq<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left == right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_ne<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialEq<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left != right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_lt<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialOrd<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left < right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_gt<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialOrd<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left > right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_lte<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialOrd<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left <= right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_gte<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: TryInto<T, Error>, T: PartialOrd<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| RuntimeValue::I32(if left >= right { 1 } else { 0 }))
			.and_then(|v| context.push_value(v))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_clz<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Integer<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.leading_zeros())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_ctz<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Integer<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.trailing_zeros())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_popcnt<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Integer<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.count_ones())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_add<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: ArithmeticOps<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.add(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_sub<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: ArithmeticOps<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.sub(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_mul<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: ArithmeticOps<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.mul(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_div<T, U>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: TransmuteInto<U>, U: ArithmeticOps<U> + TransmuteInto<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| (left.transmute_into(), right.transmute_into()))
			.map(|(left, right)| left.div(right))
			.map(|v| v.transmute_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_rem<T, U>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: TransmuteInto<U>, U: Integer<U> + TransmuteInto<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| (left.transmute_into(), right.transmute_into()))
			.map(|(left, right)| left.rem(right))
			.map(|v| v.transmute_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_and<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<<T as ops::BitAnd>::Output> + TryInto<T, Error>, T: ops::BitAnd<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.bitand(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_or<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<<T as ops::BitOr>::Output> + TryInto<T, Error>, T: ops::BitOr<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.bitor(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_xor<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<<T as ops::BitXor>::Output> + TryInto<T, Error>, T: ops::BitXor<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.bitxor(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_shl<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<<T as ops::Shl<T>>::Output> + TryInto<T, Error>, T: ops::Shl<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.shl(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_shr<T, U>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: TransmuteInto<U>, U: ops::Shr<U>, <U as ops::Shr<U>>::Output: TransmuteInto<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| (left.transmute_into(), right.transmute_into()))
			.map(|(left, right)| left.shr(right))
			.map(|v| v.transmute_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_rotl<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Integer<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.rotl(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_rotr<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Integer<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.rotr(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_abs<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.abs())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_neg<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<<T as ops::Neg>::Output> + TryInto<T, Error>, T: ops::Neg {
		context
			.pop_value_as::<T>()
			.map(|v| v.neg())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_ceil<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.ceil())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_floor<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.floor())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_trunc<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.trunc())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_nearest<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.round())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_sqrt<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_as::<T>()
			.map(|v| v.sqrt())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_min<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.min(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_max<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		context
			.pop_value_pair_as::<T>()
			.map(|(left, right)| left.max(right))
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_copysign<T>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<T> + TryInto<T, Error>, T: Float<T> {
		Err(Error::NotImplemented)
	}

	fn run_wrap<T, U>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<U> + TryInto<T, Error>, T: WrapInto<U> {
		context
			.pop_value_as::<T>()
			.map(|v| v.wrap_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_trunc_to_int<T, U, V>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<V> + TryInto<T, Error>, T: TryTruncateInto<U, Error>, U: TransmuteInto<V>,  {
		context
			.pop_value_as::<T>()
			.and_then(|v| v.try_truncate_into())
			.map(|v| v.transmute_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_extend<T, U, V>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<V> + TryInto<T, Error>, T: ExtendInto<U>, U: TransmuteInto<V> {
		context
			.pop_value_as::<T>()
			.map(|v| v.extend_into())
			.map(|v| v.transmute_into())
			.map(|v| context.push_value(v.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn run_reinterpret<T, U>(context: &mut FunctionContext) -> Result<InstructionOutcome, Error>
		where RuntimeValue: From<U>, RuntimeValue: TryInto<T, Error>, T: TransmuteInto<U> {
		context
			.pop_value_as::<T>()
			.map(TransmuteInto::transmute_into)
			.and_then(|val| context.push_value(val.into()))
			.map(|_| InstructionOutcome::RunNextInstruction)
	}

	fn execute_block(context: &mut FunctionContext, block_type: BlockType, body: &[Opcode]) -> Result<InstructionOutcome, Error> {
		debug_assert!(!context.frame_stack.is_empty());

		// run instructions
		context.position = 0;
		loop {
			// TODO: blocks ends with end => it should work with
			// If the current position is now past the end of the sequence, function return
			// execution is initiated and execution of the function is thereafter complete.
			// if context.position == body_len {
			// 	return Ok(InstructionOutcome::Next);
			// }
			let instruction = &body[context.position];
println!("=== RUNNING {:?}", instruction);
			match Interpreter::run_instruction(context, instruction)? {
				InstructionOutcome::RunInstruction => (),
				InstructionOutcome::RunNextInstruction => context.position += 1,
				InstructionOutcome::PopFrame(index) => {
					context.pop_frame()?;
					if index != 0 {
						return Ok(InstructionOutcome::PopFrame(index - 1));
					} else {
						return Ok(InstructionOutcome::RunInstruction);
					}
				},
				InstructionOutcome::Return => return Ok(InstructionOutcome::Return),
			}
		}
	}
}

impl<'a> FunctionContext<'a> {
	pub fn new(module: &'a mut ModuleInstance, value_stack: &'a mut VecDeque<RuntimeValue>, frame_stack: &'a mut VecDeque<BlockFrame>, function: &FunctionType, body: &[Opcode], args: &[RuntimeValue]) -> Result<Self, Error> {
		let mut context = FunctionContext {
			module: module,
			// The value stack begins empty.
			value_stack: value_stack,
			// The control-flow stack begins with an entry holding a label bound to the last instruction in
			// the instruction sequence, a limit value of zero, and a signature corresponding to the function's
			// return types:
			// - If the function's return type sequence is empty, its signature is void.
			// - If the function's return type sequence has exactly one element, the signature is that element.
			frame_stack: frame_stack,
			// The value of each incoming argument is copied to the local with the corresponding index, and the rest of the locals are initialized to all-zeros bit-pattern values.
			locals: Vec::from(args),
			// The current position starts at the first instruction in the function body.
			position: 0,
		};
		context.push_frame(body.len() - 1, match function.return_type() {
			Some(value_type) => BlockType::Value(value_type),
			None => BlockType::NoResult,
		})?;
		Ok(context)
	}

	pub fn module(&mut self) -> &mut ModuleInstance {
		self.module
	}

	pub fn set_local(&mut self, index: usize, value: RuntimeValue) -> Result<InstructionOutcome, Error> {
		self.locals.get_mut(index)
			.map(|local| *local = value)
			.map(|_| InstructionOutcome::RunNextInstruction)
			.ok_or(Error::Local(format!("expected to have local with index {}", index)))
	}

	pub fn get_local(&mut self, index: usize) -> Result<&RuntimeValue, Error> {
		self.locals.get(index)
			.ok_or(Error::Local(format!("expected to have local with index {}", index)))
	}

	pub fn push_value(&mut self, value: RuntimeValue) -> Result<(), Error> {
		self.value_stack.push_back(value);
		Ok(())
	}

	pub fn top_value(&mut self) -> Result<RuntimeValue, Error> {
		self.value_stack
			.back()
			.cloned()
			.ok_or(Error::ValueStack("non-empty value stack expected".into()	))
	}

	pub fn pop_value(&mut self) -> Result<RuntimeValue, Error> {
		self.value_stack
			.pop_back()
			.ok_or(Error::ValueStack("non-empty value stack expected".into()))
	}

	pub fn pop_value_as<T>(&mut self) -> Result<T, Error>
		where RuntimeValue: TryInto<T, Error> {
		self.pop_value()
			.and_then(TryInto::try_into)
	}

	pub fn pop_value_pair(&mut self) -> Result<(RuntimeValue, RuntimeValue), Error> {
		let right = self.pop_value()?;
		let left = self.pop_value()?;
		Ok((left, right))
	}

	pub fn pop_value_pair_as<T>(&mut self) -> Result<(T, T), Error>
		where RuntimeValue: TryInto<T, Error> {
		let right = self.pop_value_as()?;
		let left = self.pop_value_as()?;
		Ok((left, right))
	}

	pub fn pop_value_triple(&mut self) -> Result<(RuntimeValue, RuntimeValue, RuntimeValue), Error> {
		let right = self.pop_value()?;
		let mid = self.pop_value()?;
		let left = self.pop_value()?;
		Ok((left, mid, right))
	}

	pub fn push_frame(&mut self, position: usize, signature: BlockType) -> Result<(), Error> {
		self.frame_stack.push_back(BlockFrame {
			position: position,
			value_limit: self.value_stack.len(),
			signature: signature,
		});
		Ok(())
	}

	pub fn pop_frame(&mut self) -> Result<(), Error> {
		let frame = match self.frame_stack.pop_back() {
			Some(frame) => frame,
			None => return Err(Error::FrameStack("non-empty frame stack expected".into())),
		};

		if frame.value_limit > self.value_stack.len() {
			return Err(Error::FrameStack("non-empty frame stack expected".into()));
		}
		let frame_value = match frame.signature {
			BlockType::Value(_) => Some(self.pop_value()?),
			BlockType::NoResult => None,
		};
		self.value_stack.resize(frame.value_limit, RuntimeValue::I32(0));
		self.position = frame.position;
		if let Some(frame_value) = frame_value {
			self.push_value(frame_value)?;
		}

		Ok(())
	}
}

impl BlockFrame {
	pub fn invalid() -> Self {
		BlockFrame {
			position: usize::max_value(),
			value_limit: usize::max_value(),
			signature: BlockType::NoResult,
		}
	}
}

fn effective_address(offset: u32, align: u32) -> Result<u32, Error> {
	if align == 0 {
		Ok(offset)
	} else {
		1u32.checked_shl(align - 1)
			.and_then(|align| align.checked_add(offset))
			.ok_or(Error::Interpreter("invalid memory alignment".into()))
	}
}

fn to_little_endian_bytes<T>(number: T) -> Vec<u8> {
	unimplemented!()
}

fn from_little_endian_bytes<T>(buffer: &[u8]) -> T {
	unimplemented!()
}

#[cfg(test)]
mod tests {
	use super::super::super::elements::{ValueType, Opcodes, Opcode, BlockType, FunctionType};
	use interpreter::Error;
	use interpreter::runner::Interpreter;
	use interpreter::value::{RuntimeValue, TryInto};

	fn run_function_i32(body: &Opcodes, arg: i32) -> Result<i32, Error> {
		let function_type = FunctionType::new(vec![ValueType::I32], Some(ValueType::I32));
		Interpreter::run_function(&function_type, body.elements(), &[RuntimeValue::I32(arg)])
			.map(|v| v.unwrap().try_into().unwrap())
	}

	#[test]
	fn trap() {
		let body = Opcodes::new(vec![
			Opcode::Unreachable,							// trap
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap_err(), Error::Trap);
	}

	#[test]
	fn nop() {
		let body = Opcodes::new(vec![
			Opcode::Nop,									// nop
			Opcode::I32Const(20),							// 20
			Opcode::Nop,									// nop
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap(), 20);
	}

	#[test]
	fn if_then() {
		let body = Opcodes::new(vec![
			Opcode::I32Const(20),							// 20
			Opcode::GetLocal(0),							// read argument
			Opcode::If(BlockType::Value(ValueType::I32),	// if argument != 0
				Opcodes::new(vec![
					Opcode::I32Const(10),					//  10
					Opcode::End,							// end
				])),
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap(), 20);
		assert_eq!(run_function_i32(&body, 1).unwrap(), 10);
	}

	#[test]
	fn if_then_else() {
		let body = Opcodes::new(vec![
			Opcode::GetLocal(0),							// read argument
			Opcode::If(BlockType::Value(ValueType::I32),	// if argument != 0
				Opcodes::new(vec![
					Opcode::I32Const(10),					//  10
					Opcode::Else,							// else
					Opcode::I32Const(20),					//  20
					Opcode::End,							// end
				])),
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap(), 20);
		assert_eq!(run_function_i32(&body, 1).unwrap(), 10);
	}

	#[test]
	fn return_from_if() {
		let body = Opcodes::new(vec![
			Opcode::GetLocal(0),							// read argument
			Opcode::If(BlockType::Value(ValueType::I32),	// if argument != 0
				Opcodes::new(vec![
					Opcode::I32Const(20),					//  20
					Opcode::Return,							//  return
					Opcode::End,
				])),
			Opcode::I32Const(10),							// 10
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap(), 10);
		assert_eq!(run_function_i32(&body, 1).unwrap(), 20);
	}

	#[test]
	fn block() {
		let body = Opcodes::new(vec![
			Opcode::Block(BlockType::Value(ValueType::I32),	// mark block
				Opcodes::new(vec![
					Opcode::I32Const(10),					// 10
					Opcode::End,
				])),
			Opcode::End]);

		assert_eq!(run_function_i32(&body, 0).unwrap(), 10);
	}

	#[test]
	fn loop_block() {
		// TODO: test
/*
		let body = Opcodes::new(vec![
			Opcode::I32Const(2),									// 2
			Opcode::Loop(BlockType::Value(ValueType::I32),			// start loop
				Opcodes::new(vec![
					Opcode::GetLocal(0),							//  read argument
					Opcode::I32Const(1),							//  1
					Opcode::I32Sub,									//  argument--
					Opcode::If(BlockType::Value(ValueType::I32),	//  if argument != 0
						Opcodes::new(vec![
							Opcode::I32Const(2),					//   2
							Opcode::I32Mul,							//   prev_val * 2
							Opcode::Br(1),							//   branch to loop
							Opcode::End,							//  end (if)
						])),
					Opcode::End,									// end (loop)
				])),
			Opcode::End]);											// end (fun)

		assert_eq!(run_function_i32(&body, 2).unwrap(), 10);
*/
	}

	#[test]
	fn branch() {
		// TODO
	}

	#[test]
	fn branch_if() {
		// TODO
	}

	#[test]
	fn branch_table() {
		// TODO
	}

	#[test]
	fn drop() {
		// TODO
	}

	#[test]
	fn select() {
		// TODO
	}
}
