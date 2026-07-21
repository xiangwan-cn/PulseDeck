use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardModel {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub renderer: RendererKind,
    pub state: CardState,
    pub value: CardValue,
    pub tooltip: Option<String>,
    pub cached: bool,
    /// 文本/列表项目超过该数量时使用多列布局。
    #[serde(default)]
    pub columns_after: Option<usize>,
    #[serde(default)]
    pub columns: Option<usize>,
}

impl CardModel {
    pub fn placeholder(id: &str, title: &str, icon: &str, renderer: RendererKind) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            subtitle: None,
            icon: Some(icon.to_string()),
            renderer,
            state: CardState::Loading,
            value: CardValue::Empty,
            tooltip: None,
            cached: false,
            columns_after: None,
            columns: None,
        }
    }

    pub fn loading(mut self) -> Self {
        self.state = CardState::Loading;
        self.value = CardValue::Text("加载中...".into());
        self
    }

    pub fn error(mut self, msg: &str) -> Self {
        self.state = CardState::Error;
        self.value = CardValue::Text(format!("错误: {}", msg));
        self
    }

    pub fn unavailable(mut self, reason: &str) -> Self {
        self.state = CardState::Unavailable;
        self.value = CardValue::Text(reason.to_string());
        self
    }

    pub fn cached(mut self) -> Self {
        self.cached = true;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererKind {
    Text,
    Value,
    Progress,
    Status,
    List,
    Composite,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardState {
    Normal,
    Loading,
    Unavailable,
    Error,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CardValue {
    Text(String),
    Number {
        value: f64,
        unit: Option<String>,
        decimals: u8,
    },
    Percentage(f64),
    Status {
        label: String,
        level: StatusLevel,
    },
    List(Vec<ListItem>),
    Composite(Vec<CardField>),
    Empty,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusLevel {
    Good,
    Normal,
    Warning,
    Critical,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub label: String,
    pub value: String,
    pub level: Option<StatusLevel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardField {
    pub label: String,
    pub value: String,
    pub level: Option<StatusLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardAction {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
}
