#[derive(Clone, Copy)]
pub struct Palette {
    pub light: &'static str,
    pub middle: &'static str,
    pub dark: &'static str,
}
pub fn get(series: &str) -> Palette {
    let (light, middle, dark) = match series {
        "IIDX RED" => ("#ff526b", "#77162d", "#160610"),
        "HAPPY SKY" => ("#b7eeff", "#28749d", "#071f34"),
        "DistorteD" => ("#f2f4f9", "#5f6475", "#08090e"),
        "GOLD" => ("#ffe5a0", "#987131", "#181007"),
        "DJ TROOPERS" => ("#c2d98b", "#546447", "#11170d"),
        "EMPRESS" => ("#ff9cda", "#823b72", "#210b20"),
        "SIRIUS" => ("#d6efff", "#365b91", "#0a112c"),
        "Resort Anthem" => ("#ffb75c", "#aa6437", "#102633"),
        "Lincle" => ("#7af5ff", "#2b88ad", "#081f3b"),
        "tricoro" => ("#e5f4ff", "#41619b", "#211a30"),
        "SPADA" => ("#ff7665", "#883333", "#1b070a"),
        "PENDUAL" => ("#ffc0f0", "#795393", "#191027"),
        "copula" => ("#ffe96b", "#6d926d", "#0a2223"),
        "SINOBUZ" => ("#b8dbda", "#42625f", "#0c161a"),
        "CANNON BALLERS" => ("#ff8679", "#687986", "#11171d"),
        "Rootage" => ("#e8ca81", "#776039", "#22130e"),
        "HEROIC VERSE" => ("#bab2ff", "#6254b0", "#15102e"),
        "BISTROVER" => ("#ffd3a0", "#b27950", "#123331"),
        "CastHour" => ("#ffb268", "#477387", "#10223a"),
        "RESIDENT" => ("#65e2ff", "#285998", "#0c0e21"),
        "EPOLIS" => ("#d7f600", "#7c8d36", "#15191b"),
        "Pinky Crush" => ("#ff9bdb", "#7970ac", "#271738"),
        "Sparkle Shower" => ("#edff91", "#84a960", "#142b20"),
        "ZINRAI" => ("#c8a8ff", "#675599", "#151024"),
        _ => ("#78deff", "#23598d", "#061326"),
    };
    Palette {
        light,
        middle,
        dark,
    }
}
