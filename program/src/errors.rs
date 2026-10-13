use anchor_lang::prelude::*;

#[error_code]
pub enum PadError {
    #[msg("bad argument")]
    BadArgs,
    #[msg("string too long")]
    TooLong,
    #[msg("fee split does not add up")]
    BadFees,
    #[msg("account does not belong to this group")]
    WrongGroup,
    #[msg("this group is closed to launches")]
    LaunchClosed,
    #[msg("group budget cannot cover this launch")]
    BudgetExhausted,
    #[msg("launch is not pending")]
    NotPending,
    #[msg("launch is not claimed")]
    NotClaimed,
    #[msg("wrong claim secret")]
    BadSecret,
    #[msg("claim window still open")]
    NotExpired,
    #[msg("a traded token never expires")]
    HasTrades,
    #[msg("slippage: output below minimum")]
    Slippage,
    #[msg("not enough tokens left on the curve")]
    SoldOut,
    #[msg("reserve too small")]
    Reserve,
    #[msg("nothing to withdraw")]
    Nothing,
    #[msg("nickname already set")]
    NicknameSet,
    #[msg("math overflow")]
    Math,
    #[msg("this collection does not take minter prompts")]
    PromptNotAllowed,
    #[msg("this collection has fixed names, no nickname")]
    NamingFixed,
    #[msg("only before the first member exists")]
    NotEmpty,
}
