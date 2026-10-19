//! Cooked Animation Blueprint routing. Node indices come from serialized
//! ChildProperties, not property-name sorting or guessed montage slot names.
use serde_json::Value;
#[derive(Debug, Clone)]
pub enum Node {
    Root(usize),
    Slot(String),
    BlendByMotion { moving: usize, stationary: usize },
}
#[derive(Debug, Clone)]
pub struct AnimationGraph {
    pub nodes: Vec<Node>,
    pub root: usize,
    pub native_parent: String,
}
impl AnimationGraph {
    pub fn parse(package: &Value) -> Result<Self, String> {
        let objects = package
            .as_array()
            .ok_or("Blueprint package is not an object table")?;
        let class = objects
            .iter()
            .find(|o| o["Type"] == "AnimBlueprintGeneratedClass")
            .ok_or("not an Animation Blueprint")?;
        let defaults = objects
            .iter()
            .find(|o| {
                o["Name"]
                    .as_str()
                    .is_some_and(|n| n.starts_with("Default__"))
            })
            .ok_or("Blueprint default object missing")?;
        let properties = &defaults["Properties"];
        let children = class["ChildProperties"]
            .as_array()
            .ok_or("Blueprint node layout missing")?;
        let fields = children
            .iter()
            .filter(|p| {
                p["Struct"]["ObjectName"]
                    .as_str()
                    .is_some_and(|n| n.contains("'AnimNode_"))
            })
            .collect::<Vec<_>>();
        if fields.is_empty() || fields.len() > 256 {
            return Err("invalid Blueprint node count".into());
        }
        let link = |v: &Value| -> Result<usize, String> {
            v["LinkID"]
                .as_u64()
                .filter(|n| *n < (fields.len() as u64))
                .map(|n| n as usize)
                .ok_or("Blueprint pose link outside graph".into())
        };
        let mut nodes = Vec::new();
        let mut root = None;
        for (index, field) in fields.iter().enumerate() {
            let name = field["Name"]
                .as_str()
                .ok_or("Blueprint node name missing")?;
            let value = &properties[name];
            let kind = field["Struct"]["ObjectName"].as_str().unwrap_or("");
            if kind.contains("'AnimNode_Root'") {
                root = Some(index);
                nodes.push(Node::Root(link(&value["Result"])?));
            } else if kind.contains("'AnimNode_Slot'") {
                nodes.push(Node::Slot(
                    value["SlotName"]
                        .as_str()
                        .ok_or("Blueprint slot name missing")?
                        .into(),
                ));
            } else if kind.contains("'AnimNode_BlendListByBool'") {
                let poses = value["BlendPose"]
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or("invalid Blueprint boolean pose blend")?;
                nodes.push(Node::BlendByMotion {
                    moving: link(&poses[0])?,
                    stationary: link(&poses[1])?,
                });
            } else {
                return Err(format!("unported Animation Blueprint node {kind}"));
            }
        }
        Ok(Self {
            nodes,
            root: root.ok_or("Blueprint root missing")?,
            native_parent: class["SuperStruct"]["ObjectName"]
                .as_str()
                .unwrap_or("")
                .into(),
        })
    }
    /// FortPropAnimInstance's stationary/moving input is supplied explicitly by
    /// the importer. This does not execute the native parent class's event code.
    pub fn slot(&self, moving: bool) -> Result<&str, String> {
        let mut index = self.root;
        for _ in 0..self.nodes.len() {
            match self
                .nodes
                .get(index)
                .ok_or("Blueprint node outside graph")?
            {
                Node::Root(next) => index = *next,
                Node::Slot(name) => return Ok(name),
                Node::BlendByMotion {
                    moving: yes,
                    stationary: no,
                } => index = if moving { *yes } else { *no },
            }
        }
        Err("Animation Blueprint cycle".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cooked_blueprint_routes_stationary_and_traversal_slots_in_source_order() {
        let fields = [
            ("Root", "AnimNode_Root"),
            ("Blend", "AnimNode_BlendListByBool"),
            ("Still", "AnimNode_Slot"),
            ("Motion", "AnimNode_Slot"),
        ]
        .map(|(name, kind)| json!({"Name":name,"Struct":{"ObjectName":format!("Class'{kind}'")}}));
        let graph=AnimationGraph::parse(&json!([{"Type":"AnimBlueprintGeneratedClass","ChildProperties":fields},{"Name":"Default__Prop","Properties":{"Root":{"Result":{"LinkID":1}},"Blend":{"BlendPose":[{"LinkID":3},{"LinkID":2}]},"Still":{"SlotName":"DefaultSlot"},"Motion":{"SlotName":"InMotion"}}}])).unwrap();
        assert_eq!(graph.slot(false).unwrap(), "DefaultSlot");
        assert_eq!(graph.slot(true).unwrap(), "InMotion");
    }
}
