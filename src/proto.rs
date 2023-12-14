use std::collections::HashMap;

use crate::ast::{BinOp, UnOp};
use crate::consts::Const;
use crate::opcodes::{Instruction, OpCode};

pub struct LocalVal {
    name: String,
}

pub struct UpVal {}

pub struct Proto {
    pub stack_size: u32,
    pub param_count: u32,
    pub code: Vec<Instruction>,
    pub consts: Vec<Const>,
    pub const_map: HashMap<Const, u32>,
    pub local_vars: Vec<LocalVal>,
    pub up_vars: Vec<UpVal>,
    pub protos: Vec<Proto>,
}

impl Default for Proto {
    fn default() -> Self {
        Proto {
            stack_size: 2,
            param_count: 0,
            code: Vec::new(),
            consts: Vec::new(),
            const_map: HashMap::new(),
            local_vars: Vec::new(),
            up_vars: Vec::new(),
            protos: Vec::new(),
        }
    }
}

impl Proto {
    pub fn open(&mut self) {}

    pub fn close(&mut self) {
        self.code_return(0, 0);
    }

    pub fn code_return(&mut self, first: u32, nret: u32) -> usize {
        self.code
            .push(Instruction::create_ABC(OpCode::Return, first, nret + 1, 0));
        self.code.len() - 1
    }

    pub fn code_nil(&mut self, start_reg: u32, n: u32) -> usize {
        // TODO : optimize for duplicate LoadNil
        self.code.push(Instruction::create_ABC(
            OpCode::LoadNil,
            start_reg,
            n - 1,
            0,
        ));
        self.code.len() - 1
    }

    pub fn code_bool(&mut self, reg: u32, v: bool, pc: u32) -> usize {
        self.code.push(Instruction::create_ABC(
            OpCode::LoadBool,
            reg,
            if v { 1 } else { 0 },
            pc,
        ));
        self.code.len() - 1
    }

    pub fn code_const(&mut self, reg_index: u32, const_index: u32) -> usize {
        self.code.push(Instruction::create_ABx(
            OpCode::LoadK,
            reg_index,
            const_index,
        ));
        self.code.len() - 1
    }

    pub fn code_move(&mut self, reg: u32, src: u32) -> usize {
        self.code
            .push(Instruction::create_ABC(OpCode::Move, reg, src, 0));
        self.code.len() - 1
    }

    pub fn code_bin_op(&mut self, op: &BinOp, target: u32, left: u32, right: u32) -> usize {
        let op_code = match op {
            BinOp::Add(_) => OpCode::Add,
            BinOp::Minus(_) => OpCode::Sub,
            BinOp::Mul(_) => OpCode::Mul,
            BinOp::Mod(_) => OpCode::Mod,
            BinOp::Pow(_) => OpCode::Pow,
            BinOp::Div(_) => OpCode::Div,
            BinOp::IDiv(_) => OpCode::IDiv,
            BinOp::BAnd(_) => OpCode::BAdd,
            BinOp::BOr(_) => OpCode::BOr,
            BinOp::BXor(_) => OpCode::BXor,
            BinOp::Shl(_) => OpCode::Shl,
            BinOp::Shr(_) => OpCode::Shr,
            BinOp::Concat(_) => OpCode::Concat,
            _ => unreachable!(),
        };
        self.code
            .push(Instruction::create_ABC(op_code, target, left, right));
        self.code.len() - 1
    }

    pub fn code_comp(&mut self, op: &BinOp, left: u32, right: u32) -> usize {
        let op_code = match op {
            BinOp::Lt(_) | BinOp::Gt(_) => OpCode::Lt,
            BinOp::Ne(_) | BinOp::Eq(_) => OpCode::Eq,
            BinOp::Le(_) | BinOp::Ge(_) => OpCode::Le,
            _ => unreachable!(),
        };
        let cond = match op {
            BinOp::Ne(_) => 0,
            _ => 1,
        };
        self.code
            .push(Instruction::create_ABC(op_code, cond, left, right));
        self.code.len() - 1
    }

    pub fn code_un_op(&mut self, op: &UnOp, target: u32, src: u32) -> usize {
        let op_code = match op {
            UnOp::Minus(_) => OpCode::Unm,
            UnOp::BNot(_) => OpCode::BNot,
            UnOp::Not(_) => OpCode::Not,
            UnOp::Len(_) => OpCode::Len,
            _ => unimplemented!(),
        };
        self.code
            .push(Instruction::create_ABC(op_code, target, src, 0));
        self.code.len() - 1
    }

    pub fn code_jmp(&mut self, offset: i32, upvars: u32) -> usize {
        self.code
            .push(Instruction::create_AsBx(OpCode::Jmp, upvars, offset));
        self.code.len() - 1
    }

    pub fn fix_cond_jump_pos(&mut self, true_pos: usize, false_pos: usize, pc: usize) {
        let instruction = self.get_instruction(pc);
        let pos = if instruction.get_arg_A() == 0 {
            true_pos
        } else {
            false_pos
        };
        instruction.set_arg_sBx(pos as i32 - pc as i32 - 1);
    }

    pub fn fix_jump_pos(&mut self, pos: usize, pc: usize) {
        let instruction = self.get_instruction(pc);
        instruction.set_arg_sBx(pos as i32 - pc as i32 - 1);
    }

    pub fn code_test_set(&mut self, set: u32, test: u32, to_test: u32) {
        self.code
            .push(Instruction::create_ABC(OpCode::TestSet, set, test, to_test));
    }

    pub fn add_local_var(&mut self, name: &str) {
        self.local_vars.push(LocalVal {
            name: name.to_string(),
        });
    }

    pub fn get_local_var(&self, name: &str) -> Option<u32> {
        self.local_vars
            .iter()
            .position(|var| var.name == name)
            .map(|i| i as u32)
    }

    pub fn add_const(&mut self, k: Const) -> u32 {
        match self.const_map.get(&k) {
            Some(index) => *index,
            None => {
                let index = self.consts.len();
                self.consts.push(k.clone());
                self.const_map.insert(k, index as u32);
                index as u32
            }
        }
    }

    // save result to target reg
    pub fn save(&mut self, target: u32) -> usize {
        let last = self.code.last_mut();
        if let Some(code) = last {
            code.save(target);
        }
        self.code.len() - 1
    }

    pub fn get_instruction(&mut self, index: usize) -> &mut Instruction {
        &mut self.code[index]
    }
}

use std::fmt;
impl fmt::Debug for Proto {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f)?;

        writeln!(f, "stack size : {}", self.stack_size)?;

        writeln!(f, "consts :")?;
        for (i, k) in self.consts.iter().enumerate() {
            writeln!(
                f,
                "| {:<5} | {:<10} |",
                i,
                match k {
                    Const::Int(i) => i.to_string(),
                    Const::Float(f) => f.to_string(),
                    Const::Str(s) => format!("\"{}\"", s.clone()),
                }
            )?;
        }

        writeln!(f, "locals :")?;
        for (i, local) in self.local_vars.iter().enumerate() {
            writeln!(f, "| {:<5} | {:<10} |", i, local.name)?;
        }

        writeln!(f, "instructions :")?;
        writeln!(
            f,
            "| {:<5} | {:<10} | {:<5} | {:<5} | {:<5} |",
            "line", "OP", "A", "B", "C"
        )?;
        for (i, instruction) in self.code.iter().enumerate() {
            writeln!(f, "| {:<5} {:?}", i + 1, instruction)?;
        }

        Ok(())
    }
}

#[derive(Default)]
pub struct ProtoContext {
    pub reg_top: u32,
    pub proto: Proto,
}

impl ProtoContext {
    pub fn check_stack(&mut self, n: u32) {
        let new_stack = self.reg_top + n;
        if new_stack > self.proto.stack_size {
            self.proto.stack_size = new_stack;
        }
    }

    pub fn reserve_regs(&mut self, n: u32) -> u32 {
        self.check_stack(n);
        let index = self.reg_top;
        self.reg_top += n;
        index
    }

    pub fn get_reg_top(&self) -> u32 {
        self.reg_top
    }

    pub fn free_reg(&mut self, n: u32) {
        self.reg_top -= n;
    }
}
