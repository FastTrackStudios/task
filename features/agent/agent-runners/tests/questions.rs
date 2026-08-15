//! The grill queue: asking, seeing, answering — and the rule that an
//! agent never answers itself.

use agent_proto::question::{AskQuestion, Question, QuestionAnswer, QuestionOption};
use agent_proto::service::questions::Questions;
use agent_runners::{Migrator, QuestionStore};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

async fn store() -> QuestionStore {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    QuestionStore::new(conn)
}

fn question(text: &str) -> Question {
    Question {
        id: Uuid::new_v4().to_string(),
        header: "Schema".into(),
        text: text.into(),
        options: vec![
            QuestionOption {
                label: "table".into(),
                description: "a new table".into(),
                preview: String::new(),
            },
            QuestionOption {
                label: "column".into(),
                description: "a column on the existing one".into(),
                preview: String::new(),
            },
        ],
        multi_select: false,
    }
}

fn ask(ticket: Uuid) -> AskQuestion {
    AskQuestion {
        ticket,
        run: Some(Uuid::new_v4()),
        questions: vec![question("Table or column?")],
    }
}

#[tokio::test]
async fn asking_puts_the_question_on_the_grill_queue() {
    let s = store().await;
    let ticket = Uuid::new_v4();
    let q = s.ask_question(ask(ticket)).await.unwrap();

    assert!(q.resolved_at.is_none());
    let queue = s.unresolved_questions().await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, q.id);
    assert_eq!(queue[0].questions[0].text, "Table or column?");
}

#[tokio::test]
async fn a_question_knows_which_ticket_it_blocks() {
    let s = store().await;
    let ticket = Uuid::new_v4();
    let q = s.ask_question(ask(ticket)).await.unwrap();

    assert_eq!(s.question_ticket(q.id).await.unwrap(), Some(ticket));
    assert_eq!(s.questions_for_ticket(ticket).await.unwrap().len(), 1);
    assert!(
        s.questions_for_ticket(Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn answering_resolves_it_and_clears_the_queue() {
    let s = store().await;
    let ticket = Uuid::new_v4();
    let q = s.ask_question(ask(ticket)).await.unwrap();
    let qid = q.questions[0].id.clone();

    let answered = s
        .answer_question(
            q.id.clone(),
            vec![QuestionAnswer {
                question_id: qid,
                selected: vec!["column".into()],
                notes: "cheaper migration".into(),
            }],
        )
        .await
        .unwrap();

    assert!(answered.resolved_at.is_some());
    assert_eq!(answered.answers[0].selected, vec!["column".to_string()]);
    assert!(s.unresolved_questions().await.unwrap().is_empty());
    assert!(s.questions_for_ticket(ticket).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_empty_answer_is_refused() {
    // Resolving with nothing would be the agent answering itself by
    // another name.
    let s = store().await;
    let q = s.ask_question(ask(Uuid::new_v4())).await.unwrap();
    assert!(s.answer_question(q.id.clone(), vec![]).await.is_err());
    assert_eq!(
        s.unresolved_questions().await.unwrap().len(),
        1,
        "a refused answer must leave the question standing"
    );
}

#[tokio::test]
async fn answering_twice_is_a_conflict_not_an_overwrite() {
    // The first answer is the one the agent acted on.
    let s = store().await;
    let q = s.ask_question(ask(Uuid::new_v4())).await.unwrap();
    let qid = q.questions[0].id.clone();
    let answer = |sel: &str| {
        vec![QuestionAnswer {
            question_id: qid.clone(),
            selected: vec![sel.into()],
            notes: String::new(),
        }]
    };

    s.answer_question(q.id.clone(), answer("table"))
        .await
        .unwrap();
    assert!(s.answer_question(q.id.clone(), answer("column")).await.is_err());

    let stored = s.question_ticket(q.id).await;
    assert!(stored.is_ok(), "the original must survive the second attempt");
}

#[tokio::test]
async fn a_question_with_no_questions_is_refused() {
    let s = store().await;
    let empty = AskQuestion {
        ticket: Uuid::new_v4(),
        run: None,
        questions: vec![],
    };
    assert!(s.ask_question(empty).await.is_err());
}

#[tokio::test]
async fn several_tickets_keep_their_own_questions() {
    let s = store().await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    s.ask_question(ask(a)).await.unwrap();
    s.ask_question(ask(a)).await.unwrap();
    s.ask_question(ask(b)).await.unwrap();

    assert_eq!(s.questions_for_ticket(a).await.unwrap().len(), 2);
    assert_eq!(s.questions_for_ticket(b).await.unwrap().len(), 1);
    assert_eq!(s.unresolved_questions().await.unwrap().len(), 3);
}

#[tokio::test]
async fn the_run_that_raised_it_is_recorded_so_the_answer_can_resume() {
    let s = store().await;
    let run = Uuid::new_v4();
    let q = s
        .ask_question(AskQuestion {
            ticket: Uuid::new_v4(),
            run: Some(run),
            questions: vec![question("Which one?")],
        })
        .await
        .unwrap();

    assert_eq!(q.session_id, run.to_string());
    assert_eq!(
        s.list_pending_questions(run.to_string()).await.unwrap().len(),
        1
    );
    assert!(
        s.list_pending_questions(Uuid::new_v4().to_string())
            .await
            .unwrap()
            .is_empty()
    );
}
