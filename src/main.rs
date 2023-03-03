use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};

use plotters::prelude::*;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use deribit::models::PublicSubscribeRequest;
use deribit::models::*;
use futures_util::stream::StreamExt;
use optiors::prelude::*;
use rust_decimal::prelude::*;


fn get_maturity_datetime(date_str: String) -> DateTime<Utc> {
    let date = NaiveDate::parse_from_str(&date_str, "%d%b%y").unwrap();
    let datetime = date.and_hms_opt(8, 0, 0).unwrap();
    let datetime_utc = Utc.from_utc_datetime(&datetime);
    datetime_utc
}

async fn connect_option(
    option_board: Arc<Mutex<OptionBoard<OptionTick>>>,
) {
    let drb = deribit::DeribitBuilder::default()
        .build()
        .expect("Cannot create deribit client");
    let (mut client, mut subscription) = drb.connect().await.unwrap();
    let req = deribit::models::GetInstrumentsRequest {
        currency: Currency::BTC,
        kind: Some(AssetKind::Option),
        expired: None,
    };
    let response = client.call(req).await.unwrap().await.unwrap();
    let mut instruments_list = Vec::new();
    for instrument in response {
        let subscribe_endpoint: String = format!("ticker.{}.100ms", instrument.instrument_name);
        instruments_list.push(subscribe_endpoint);
    }
	// instruments_list.push("ticker.BTC-10MAR23-22500-C.100ms".to_string());

	let req = PublicSubscribeRequest::new(&instruments_list);
    let _ = client.call(req).await.unwrap();

    while let Some(message) = subscription.next().await {
        let message = message.unwrap().params;
        let receive_data = if let SubscriptionParams::Subscription(data) = message {
            data
        } else {
            panic!("Wrong data type")
        };
        let channel = if let SubscriptionData::Ticker(ticker_) = receive_data {
            ticker_.data
        } else {
            panic!("Not a book")
        };
        let instrument_name = channel.instrument_name.split("-").collect::<Vec<&str>>();

        let maturity = get_maturity_datetime(instrument_name[1].to_string());
		let strike = instrument_name[2].parse::<Decimal>().unwrap();
		let option_type = if instrument_name[3] == "C" {
			OptionType::Call
		} else {
			OptionType::Put
		};

		let ask_iv = channel.ask_iv.unwrap();
		let bid_iv = channel.bid_iv.unwrap();
		let mid_iv = if ask_iv < FloatType::EPSILON && bid_iv < FloatType::EPSILON{
			continue;
		}
		else if ask_iv < FloatType::EPSILON{
			bid_iv
		} else if bid_iv < FloatType::EPSILON{
			ask_iv
		} else{
			(bid_iv + ask_iv) / 2.
		} / 100.;
		let open_interest = channel.open_interest;
		let asset_price = channel.underlying_price.unwrap();

		let option_tick = OptionTick::builder().strike(strike).maturity(maturity)
			.asset_price(asset_price).option_type(option_type)
			.option_value(OptionValue::ImpliedVolatility(mid_iv))
			.additional_data(AdditionalOptionData::builder().open_interest(open_interest).build())
			.build();

        let mut locked_option_board = option_board.lock().unwrap();
		locked_option_board.upsert(option_tick);
		

	}
}

async fn view_option(
    option_board: Arc<Mutex<OptionBoard<OptionTick>>>,
) {
    sleep(Duration::from_secs(10)).await;
    loop {

        let board = option_board.lock().unwrap().clone();
		let option_chain = board.sort_by_maturity().get(2).otm();
		let (strikes, values) = option_chain.map_to_vec(OptionTick::iv);
		let _ = plot(strikes, values, "iv");

		let (strikes, values) = option_chain.map_to_vec(OptionTick::gamma);
		let _ = plot(strikes, values, "gamma");

		let (strikes, values) = option_chain.map_to_vec(OptionTick::color);
		let _ = plot(strikes, values, "color");

		dbg!(option_chain.atm().delta());
		dbg!(option_chain.atm().gamma());
		dbg!(option_chain.atm().vega());
		// dbg!(option_chain.call_25delta().delta());
		// dbg!(front_month.call_25delta().iv() - front_month.put_25delta().iv());
		dbg!(option_chain.delta_exposure().unwrap());
		dbg!(option_chain.gamma_exposure().unwrap());
		println!("---------------------------------");

		sleep(Duration::from_secs(10)).await;

    }
}


fn plot(x:Vec<f64>, y:Vec<f64>, name:&str) -> Result<(), Box<dyn std::error::Error>> {
	let (y_min, y_max) = y.iter()
        .fold(
            (0.0/0.0, 0.0/0.0),
            |(m,n), v| (v.min(m), v.max(n))
        );

	let file_name = format!("{}.png", name);
	let root = BitMapBackend::new(&file_name, (640, 480)).into_drawing_area();
	root.fill(&WHITE)?;
	// グラフ領域の作成
	let mut chart = ChartBuilder::on(&root)
		.caption(name, ("sans-serif", 50).into_font())
		.margin(5)
		.x_label_area_size(30)
		.y_label_area_size(30)
		.build_cartesian_2d(                // x軸とy軸の数値の範囲を指定する
			*x.first().unwrap()..*x.last().unwrap(), // x軸の範囲
			y_min..y_max                               // y軸の範囲
		)?;

	// 軸やグリッドの設定
	chart.configure_mesh().draw()?;
	// 曲線の描画
	chart.draw_series(LineSeries::new(x.iter().zip(y.iter()).map(|(x,y)| (*x,*y)), &RED))?;

	Ok(())



}


#[tokio::main]
async fn main() {
    let mutex_board = Arc::new(Mutex::new(OptionBoard::<OptionTick>::new()));
    let conn_option = tokio::spawn(connect_option(mutex_board.clone()));
    let view = tokio::spawn(view_option(mutex_board.clone()));

	conn_option.await.unwrap();
    view.await.unwrap();
}
