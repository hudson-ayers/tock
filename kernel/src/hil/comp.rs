//! Interface for comparing the voltage of two pins in differential mode. 
//! Reference pin is derived directly from AINx pins.

/// Enum for the input pin, can be AIN0-AIN7 
pub enum Pin {
    AIN0,
    AIN1,
    AIN2,
    AIN3,
    AIN4,
    AIN5,
    AIN6,
    AIN7,
    ERROR,
}

/// Function to return the pin number corresponding to the pin name
pub fn return_pin_num(pin: Pin) -> u32 {
    match pin {
        Pin::AIN0 => 0,
        Pin::AIN1 => 1,
        Pin::AIN2 => 2,
        Pin::AIN3 => 3,
        Pin::AIN4 => 4,
        Pin::AIN5 => 5,
        Pin::AIN6 => 6,
        Pin::AIN7 => 7,
        Pin::ERROR => 8,
    }
}

/// Function to return the Pin enum corresponding to pin number
pub fn return_pin_enum(pin_num: u32) -> Pin {
    match pin_num {
        0 => Pin::AIN0,
        1 => Pin::AIN1,
        2 => Pin::AIN2,
        3 => Pin::AIN3,
        4 => Pin::AIN4,
        5 => Pin::AIN5,
        6 => Pin::AIN6,
        7 => Pin::AIN7,
        _ => Pin::ERROR, 
    }
}

/// Enum for single-ended mode reference pin, can be variations of VREF
pub enum RefPin {
    AIN0,
    AIN1,
    AIN2,
    AIN3,
    AIN4,
    AIN5,
    AIN6,
    AIN7,
    Int1V2,
    Int1V8,
    Int2V4,
    VDD,
    ARef,
    ERROR,
}

/// Function to return the reference pin number corresponding to the reference pin name
pub fn return_ref_pin_num(ref_pin: RefPin) -> u32 {
    match ref_pin {
        RefPin::AIN0 => 0,
        RefPin::AIN1 => 1,
        RefPin::AIN2 => 2,
        RefPin::AIN3 => 3,
        RefPin::AIN4 => 4,
        RefPin::AIN5 => 5,
        RefPin::AIN6 => 6,
        RefPin::AIN7 => 7,
        RefPin::Int1V2 => 8,
        RefPin::Int1V8 => 9,
        RefPin::Int2V4 => 10,
        RefPin::VDD => 11,
        RefPin::ARef => 12,
        RefPin::ERROR => 13,
    }
}

/// Function to return the RefPin enum corresponding to refernce pin number
pub fn return_ref_pin_enum(ref_pin_num: u32) -> RefPin {
    match ref_pin_num {
        0 => RefPin::AIN0,
        1 => RefPin::AIN1,
        2 => RefPin::AIN2,
        3 => RefPin::AIN3,
        4 => RefPin::AIN4,
        5 => RefPin::AIN5,
        6 => RefPin::AIN6,
        7 => RefPin::AIN7,
        8 => RefPin::Int1V2,
        9 => RefPin::Int1V8,
        10 => RefPin::Int2V4,
        11 => RefPin::VDD,
        12 => RefPin::ARef,
        _ => RefPin::ERROR, 
    }
}

/// Enum for main operation modes -- single-ended mode, differential mode
pub enum OpModes {
    SE,
    Diff,
}

/// Function to return the main operation mode number corresponding to the mode name
pub fn return_op_mode(opmode: OpModes) -> u32 {
    match opmode {
        OpModes::SE => 0,
        OpModes::Diff => 1,
    }
}

/// Enum for speed and power modes
pub enum SPModes {
    Low,
    Normal,
    High,
}

/// Function to return the speed and power operation mode number corresponding to the mode name
pub fn return_sp_mode(spmode: SPModes) -> u32 {
    match spmode {
        SPModes::Low => 0,
        SPModes::Normal => 1,
        SPModes::High => 2,
    }
}

pub trait AnalogComparator<'a,I,R> {
    /// Sets the input pin for comparison
    fn set_input(&self, input_pin: I);

    /// Sets the reference pin value from AIN0 to AIN7
    fn set_reference(&self, ref_pin: R);

    // Sets both input and reference pin for comparison
    fn set_both(&self, input_pin: I, ref_pin: R);

    /// Performs pin voltage comparison
    fn start_comparing(&self, input_pin: I, ref_pin: R, rising: bool, mode: OpModes);

    /// Sets the client for the comparator
    fn set_client(&self, client: &'a Client<I,R>);

    /// Stops the comparator
    fn stop(&self);
}

// Pass in pin type as input (pin type: enum that matches to underlying pins)
pub trait Client<I,R> {
    /// callback to client, returns information about the comparison 
    fn event(&self, rising: bool, input: I, reference: R);
}
